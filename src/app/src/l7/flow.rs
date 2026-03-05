use anyhow::Result;
use vane_engine::engine::executor;
use vane_engine::engine::interfaces::{ConnectionObject, ProcessingStep, TerminatorResult};

use crate::context::ApplicationContext;
use crate::l7::container::Container;

pub async fn execute_l7(
	step: &ProcessingStep,
	container: &mut Container,
	flow_path: String,
) -> Result<TerminatorResult> {
	let mut context = ApplicationContext { container };
	let conn = ConnectionObject::Virtual("L7_Managed_Context".into());
	executor::execute(step, &mut context, conn, flow_path).await
}
