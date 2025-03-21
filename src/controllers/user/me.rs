use crate::auth::AuthCheck;
use std::collections::HashMap;

use crate::controllers::frontend_prelude::*;

use crate::controllers::helpers::*;

use crate::controllers::helpers::pagination::{Paginated, PaginationOptions};
use crate::models::{
    CrateOwner, Email, Follow, NewEmail, OwnerKind, User, Version, VersionOwnerAction,
};
use crate::schema::{crate_owners, crates, emails, follows, users, versions};
use crate::views::{EncodableMe, EncodablePrivateUser, EncodableVersion, OwnedCrate};

/// Handles the `GET /me` route.
pub async fn me(app: AppState, req: Parts) -> AppResult<Json<EncodableMe>> {
    conduit_compat(move || {
        let conn = &mut *app.db_read_prefer_primary()?;
        let user_id = AuthCheck::only_cookie().check(&req, conn)?.user_id();

        let (user, verified, email, verification_sent): (User, Option<bool>, Option<String>, bool) =
            users::table
                .find(user_id)
                .left_join(emails::table)
                .select((
                    users::all_columns,
                    emails::verified.nullable(),
                    emails::email.nullable(),
                    emails::token_generated_at.nullable().is_not_null(),
                ))
                .first(conn)?;

        let owned_crates = CrateOwner::by_owner_kind(OwnerKind::User)
            .inner_join(crates::table)
            .filter(crate_owners::owner_id.eq(user_id))
            .select((crates::id, crates::name, crate_owners::email_notifications))
            .order(crates::name.asc())
            .load(conn)?
            .into_iter()
            .map(|(id, name, email_notifications)| OwnedCrate {
                id,
                name,
                email_notifications,
            })
            .collect();

        let verified = verified.unwrap_or(false);
        let verification_sent = verified || verification_sent;
        Ok(Json(EncodableMe {
            user: EncodablePrivateUser::from(user, email, verified, verification_sent),
            owned_crates,
        }))
    })
    .await
}

/// Handles the `GET /me/updates` route.
pub async fn updates(app: AppState, req: Parts) -> AppResult<Json<Value>> {
    conduit_compat(move || {
        let conn = &mut app.db_read_prefer_primary()?;
        let auth = AuthCheck::only_cookie().check(&req, conn)?;
        let user = auth.user();

        let followed_crates = Follow::belonging_to(user).select(follows::crate_id);
        let query = versions::table
            .inner_join(crates::table)
            .left_outer_join(users::table)
            .filter(crates::id.eq_any(followed_crates))
            .order(versions::created_at.desc())
            .select((
                versions::all_columns,
                crates::name,
                users::all_columns.nullable(),
            ))
            .pages_pagination(PaginationOptions::builder().gather(&req)?);
        let data: Paginated<(Version, String, Option<User>)> = query.load(conn)?;
        let more = data.next_page_params().is_some();
        let versions = data.iter().map(|(v, _, _)| v).cloned().collect::<Vec<_>>();
        let data = data
            .into_iter()
            .zip(VersionOwnerAction::for_versions(conn, &versions)?)
            .map(|((v, cn, pb), voas)| (v, cn, pb, voas));

        let versions = data
            .into_iter()
            .map(|(version, crate_name, published_by, actions)| {
                EncodableVersion::from(version, &crate_name, published_by, actions)
            })
            .collect::<Vec<_>>();

        Ok(Json(json!({
            "versions": versions,
            "meta": { "more": more },
        })))
    })
    .await
}

/// Handles the `PUT /users/:user_id` route.
pub async fn update_user(
    state: AppState,
    Path(param_user_id): Path<i32>,
    req: BytesRequest,
) -> AppResult<Response> {
    conduit_compat(move || {
        use self::emails::user_id;
        use diesel::insert_into;

        let conn = &mut state.db_write()?;

        let auth = AuthCheck::default().check(&req, conn)?;
        let user = auth.user();

        // need to check if current user matches user to be updated
        if user.id != param_user_id {
            return Err(bad_request("current user does not match requested user"));
        }

        #[derive(Deserialize)]
        struct UserUpdate {
            user: User,
        }

        #[derive(Deserialize)]
        struct User {
            email: Option<String>,
        }

        let user_update: UserUpdate =
            serde_json::from_slice(req.body()).map_err(|_| bad_request("invalid json request"))?;

        let user_email = match &user_update.user.email {
            Some(email) => email.trim(),
            None => return Err(bad_request("empty email rejected")),
        };

        if user_email.is_empty() {
            return Err(bad_request("empty email rejected"));
        }

        conn.transaction::<_, BoxedAppError, _>(|conn| {
            let new_email = NewEmail {
                user_id: user.id,
                email: user_email,
            };

            let token: String = insert_into(emails::table)
                .values(&new_email)
                .on_conflict(user_id)
                .do_update()
                .set(&new_email)
                .returning(emails::token)
                .get_result(conn)
                .map_err(|_| server_error("Error in creating token"))?;

            // This swallows any errors that occur while attempting to send the email. Some users have
            // an invalid email set in their GitHub profile, and we should let them sign in even though
            // we're trying to silently use their invalid address during signup and can't send them an
            // email. They'll then have to provide a valid email address.
            let _ = state
                .emails
                .send_user_confirm(user_email, &user.gh_login, &token);

            Ok(())
        })?;

        ok_true()
    })
    .await
}

/// Handles the `PUT /confirm/:email_token` route
pub async fn confirm_user_email(state: AppState, Path(token): Path<String>) -> AppResult<Response> {
    conduit_compat(move || {
        use diesel::update;

        let conn = &mut *state.db_write()?;

        let updated_rows = update(emails::table.filter(emails::token.eq(&token)))
            .set(emails::verified.eq(true))
            .execute(conn)?;

        if updated_rows == 0 {
            return Err(bad_request("Email belonging to token not found."));
        }

        ok_true()
    })
    .await
}

/// Handles `PUT /user/:user_id/resend` route
pub async fn regenerate_token_and_send(
    state: AppState,
    Path(param_user_id): Path<i32>,
    req: Parts,
) -> AppResult<Response> {
    conduit_compat(move || {
        use diesel::dsl::sql;
        use diesel::update;

        let conn = &mut state.db_write()?;

        let auth = AuthCheck::default().check(&req, conn)?;
        let user = auth.user();

        // need to check if current user matches user to be updated
        if user.id != param_user_id {
            return Err(bad_request("current user does not match requested user"));
        }

        conn.transaction(|conn| {
            let email: Email = update(Email::belonging_to(user))
                .set(emails::token.eq(sql("DEFAULT")))
                .get_result(conn)
                .map_err(|_| bad_request("Email could not be found"))?;

            state
                .emails
                .send_user_confirm(&email.email, &user.gh_login, &email.token)
        })?;

        ok_true()
    })
    .await
}

/// Handles `PUT /me/email_notifications` route
pub async fn update_email_notifications(app: AppState, req: BytesRequest) -> AppResult<Response> {
    conduit_compat(move || {
        use diesel::pg::upsert::excluded;

        #[derive(Deserialize)]
        struct CrateEmailNotifications {
            id: i32,
            email_notifications: bool,
        }

        let updates: HashMap<i32, bool> =
            serde_json::from_slice::<Vec<CrateEmailNotifications>>(req.body())
                .map_err(|_| bad_request("invalid json request"))?
                .iter()
                .map(|c| (c.id, c.email_notifications))
                .collect();

        let conn = &mut *app.db_write()?;
        let user_id = AuthCheck::default().check(&req, conn)?.user_id();

        // Build inserts from existing crates belonging to the current user
        let to_insert = CrateOwner::by_owner_kind(OwnerKind::User)
            .filter(crate_owners::owner_id.eq(user_id))
            .select((
                crate_owners::crate_id,
                crate_owners::owner_id,
                crate_owners::owner_kind,
                crate_owners::email_notifications,
            ))
            .load(conn)?
            .into_iter()
            // Remove records whose `email_notifications` will not change from their current value
            .map(
                |(c_id, o_id, o_kind, e_notifications): (i32, i32, i32, bool)| {
                    let current_e_notifications = *updates.get(&c_id).unwrap_or(&e_notifications);
                    (
                        crate_owners::crate_id.eq(c_id),
                        crate_owners::owner_id.eq(o_id),
                        crate_owners::owner_kind.eq(o_kind),
                        crate_owners::email_notifications.eq(current_e_notifications),
                    )
                },
            )
            .collect::<Vec<_>>();

        // Upsert crate owners; this should only actually exectute updates
        diesel::insert_into(crate_owners::table)
            .values(&to_insert)
            .on_conflict((
                crate_owners::crate_id,
                crate_owners::owner_id,
                crate_owners::owner_kind,
            ))
            .do_update()
            .set(crate_owners::email_notifications.eq(excluded(crate_owners::email_notifications)))
            .execute(conn)?;

        ok_true()
    })
    .await
}
