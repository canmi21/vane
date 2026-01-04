/* src/layers/l7/flow.rs */

use anyhow::Result;

use crate::engine::contract::{ProcessingStep, TerminatorResult};
use crate::engine::executor;
use crate::layers::l7::container::Container;

pub async fn execute_l7(
	step: &ProcessingStep,
	container: &mut Container,
	parent_path: String,
) -> Result<TerminatorResult> {
	executor::execute_l7(step, container, parent_path).await
}
