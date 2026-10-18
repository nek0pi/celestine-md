//! CelestineMD core: vault indexing, wikilink parsing, resolution, and graph.

mod error;
mod graph;
mod index;
mod path;
mod parse;
mod resolve;
mod vault;

pub use error::CoreError;
pub use graph::{LinkGraph, NodeId};
pub use index::{Heading, NoteId, NoteMeta, VaultIndex};
pub use path::resolve_vault_path;
pub use parse::{extract_note_data, Callout, Frontmatter, InlineLink, ParsedNote, Wikilink};
pub use resolve::{resolve_link, ResolvedLink};
pub use vault::Vault;
