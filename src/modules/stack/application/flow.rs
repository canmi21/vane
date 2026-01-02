/* src/modules/stack/application/flow.rs */

use anyhow::Result;

use crate::modules::{
	flow::engine,
	plugins::core::model::{ProcessingStep, TerminatorResult},
	stack::application::container::Container,
};

pub async fn execute_l7(
	step: &ProcessingStep,
	container: &mut Container,
	parent_path: String,
) -> Result<TerminatorResult> {
	engine::execute_l7(step, container, parent_path).await
}
