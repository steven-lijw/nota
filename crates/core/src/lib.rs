//! nota-core: the UI-independent part of Nota — data model, Markdown
//! serialization, the on-disk vault and database queries.

pub mod markdown;
pub mod model;
pub mod query;
pub mod rich_text;
pub mod sample;
pub mod vault;

pub use model::*;
pub use rich_text::{MarkKind, Marks, RichText};
pub use vault::{Anchor, AppState, ChangedOnDisk, SearchHit, Vault};
