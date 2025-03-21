//! This module contains the code for the background jobs that run on the
//! crates.io backend servers.
//!
//! The `swirl` submodule contains the code for the generic background job
//! runner, and the `jobs` submodule contains the application-specific
//! background job definitions.

use self::swirl::Runner;
use std::sync::Arc;

mod environment;
pub mod jobs;
pub mod swirl;

pub use self::environment::Environment;

pub trait RunnerExt {
    fn register_crates_io_job_types(self) -> Self;
}

impl RunnerExt for Runner<Arc<Environment>> {
    fn register_crates_io_job_types(self) -> Self {
        self.register_job_type::<jobs::DailyDbMaintenance>()
            .register_job_type::<jobs::DumpDb>()
            .register_job_type::<jobs::NormalizeIndex>()
            .register_job_type::<jobs::RenderAndUploadReadme>()
            .register_job_type::<jobs::SquashIndex>()
            .register_job_type::<jobs::SyncToGitIndex>()
            .register_job_type::<jobs::SyncToSparseIndex>()
            .register_job_type::<jobs::UpdateDownloads>()
    }
}
