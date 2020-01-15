import { Factory } from 'ember-cli-mirage';

export default Factory.extend({
  name: i => `crate-${i}`,

  id() {
    return this.name;
  },

  description() {
    return `This is the description for the crate called "${this.name}"`;
  },

  downloads: i => (((i + 13) * 42) % 13) * 12345,

  documentation: null,
  homepage: null,
  repository: null,
  max_version: '1.0.0',
  newest_version: '1.0.0',

  created_at: '2010-06-16T21:30:45Z',
  updated_at: '2017-02-24T12:34:56Z',

  badges: () => [],
  categories: () => [],
  keywords: () => [],
  versions: () => [],
  _extra_downloads: () => [],
  _owner_teams: () => [],
  _owner_users: () => [],
});
