//! CommandHandler port — the seam between the transport layer and the
//! command-specific logic. One implementation per `RexCommand` variant.
//!
//! After C1, `handler/mod.rs` holds a `match`-based dispatch table. Each
//! arm calls the trait, so a new command is one `impl CommandHandler for ...`
//! + one line in the table.

use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexData};

use crate::Services;

/// One handler per `RexCommand` variant. The method is `async` because
/// most commands involve I/O (send a packet, persist client state, forward
/// to another node).
///
/// The dispatch in `handler/mod.rs` matches on `RexCommand` directly rather
/// than using a hash-map keyed by `command()`. The match gives exhaustiveness
/// checking from the compiler.
pub trait CommandHandler: Send + Sync {
    async fn handle(
        &self,
        services: &Services,
        client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()>;
}
