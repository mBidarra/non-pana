//! Núcleo do Non Panã: cofre, formatos de arquivo e modelo de domínio.
//! Ver `docs/DECISIONS.md` para o contrato de formatos.

pub mod config;
pub mod frontmatter;
pub mod fsx;
pub mod ids;
pub mod model;
pub mod text;
pub mod thread;
pub mod time;
pub mod vault;

pub use config::Config;
pub use ids::new_id;
pub use thread::{AgentMode, Event, EventBody, Material, PostView, ThreadKind, ThreadState};
pub use vault::Vault;

pub const SCHEMA: u32 = 1;
