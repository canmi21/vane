// Re-export core execute function from engine crate
pub use vane_engine::engine::executor::execute;

// L7 convenience wrapper (stays here until Step 4: vane-app extraction)
use anyhow::Result;

use crate::engine::{
	context::ApplicationContext,
	interfaces::{ConnectionObject, ProcessingStep, TerminatorResult},
};
use crate::layers::l7::container::Container;

/// Convenience wrapper for L7 flow execution.
pub async fn execute_l7(
	step: &ProcessingStep,
	container: &mut Container,
	flow_path: String,
) -> Result<TerminatorResult> {
	let mut context = ApplicationContext { container };
	let conn = ConnectionObject::Virtual("L7_Managed_Context".into());
	execute(step, &mut context, conn, flow_path).await
}
