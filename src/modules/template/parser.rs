/* src/modules/template/parser.rs */

use anyhow::{Context, Result};

/// Template AST node
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateNode {
	/// Plain text segment
	Text(String),

	/// Variable reference {{...}}
	Variable {
		/// Can contain nested nodes for concatenation/nesting
		parts: Vec<TemplateNode>,
	},
}

/// Parse template string into AST
pub fn parse_template(input: &str) -> Result<Vec<TemplateNode>> {
	let mut nodes = Vec::new();
	let mut chars = input.chars().peekable();
	let mut current_text = String::new();

	while let Some(ch) = chars.next() {
		// Check for start of variable {{
		if ch == '{' {
			if chars.peek() == Some(&'{') {
				chars.next(); // consume second {

				// Save any accumulated text
				if !current_text.is_empty() {
					nodes.push(TemplateNode::Text(current_text.clone()));
					current_text.clear();
				}

				// Parse variable content until }}
				let var_content =
					parse_variable_content(&mut chars).context("Failed to parse variable content")?;

				// Recursively parse the variable content
				let parts = parse_template(&var_content)?;

				nodes.push(TemplateNode::Variable { parts });
			} else {
				current_text.push(ch);
			}
		} else {
			current_text.push(ch);
		}
	}

	// Add remaining text
	if !current_text.is_empty() {
		nodes.push(TemplateNode::Text(current_text));
	}

	Ok(nodes)
}

/// Parse content inside {{ ... }} until }}
fn parse_variable_content(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String> {
	let mut content = String::new();
	let mut depth = 0;

	while let Some(ch) = chars.next() {
		// Check for nested {{ or closing }}
		if ch == '{' {
			if chars.peek() == Some(&'{') {
				chars.next(); // consume second {
				depth += 1;
				content.push_str("{{");
			} else {
				content.push(ch);
			}
		} else if ch == '}' {
			if chars.peek() == Some(&'}') {
				chars.next(); // consume second }

				if depth == 0 {
					// Found closing }} at current level
					return Ok(content);
				} else {
					depth -= 1;
					content.push_str("}}");
				}
			} else {
				content.push(ch);
			}
		} else {
			content.push(ch);
		}
	}

	anyhow::bail!("Unclosed variable: missing closing }}")
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Tests parsing plain text without variables.
	#[test]
	fn test_parse_plain_text() {
		let result = parse_template("plain text").unwrap();
		assert_eq!(result, vec![TemplateNode::Text("plain text".to_string())]);
	}

	/// Tests parsing simple variable.
	#[test]
	fn test_parse_simple_variable() {
		let result = parse_template("{{key}}").unwrap();
		assert_eq!(
			result,
			vec![TemplateNode::Variable {
				parts: vec![TemplateNode::Text("key".to_string())]
			}]
		);
	}

	/// Tests parsing mixed text and variables.
	#[test]
	fn test_parse_mixed() {
		let result = parse_template("before {{key}} after").unwrap();
		assert_eq!(result.len(), 3);
		assert_eq!(result[0], TemplateNode::Text("before ".to_string()));
		assert!(matches!(result[1], TemplateNode::Variable { .. }));
		assert_eq!(result[2], TemplateNode::Text(" after".to_string()));
	}

	/// Tests parsing concatenated variables.
	#[test]
	fn test_parse_concatenation() {
		let result = parse_template("{{a}}:{{b}}").unwrap();
		assert_eq!(result.len(), 3);
		assert!(matches!(result[0], TemplateNode::Variable { .. }));
		assert_eq!(result[1], TemplateNode::Text(":".to_string()));
		assert!(matches!(result[2], TemplateNode::Variable { .. }));
	}

	/// Tests parsing nested variables.
	#[test]
	fn test_parse_nested() {
		let result = parse_template("{{kv.{{proto}}_backend}}").unwrap();
		assert_eq!(result.len(), 1);

		if let TemplateNode::Variable { parts } = &result[0] {
			assert_eq!(parts.len(), 3);
			assert_eq!(parts[0], TemplateNode::Text("kv.".to_string()));
			assert!(matches!(parts[1], TemplateNode::Variable { .. }));
			assert_eq!(parts[2], TemplateNode::Text("_backend".to_string()));
		} else {
			panic!("Expected Variable node");
		}
	}

	/// Tests parsing empty template.
	#[test]
	fn test_parse_empty() {
		let result = parse_template("").unwrap();
		assert_eq!(result, vec![]);
	}

	/// Tests unclosed variable error.
	#[test]
	fn test_parse_unclosed_variable() {
		let result = parse_template("{{key");
		assert!(result.is_err());
	}

	/// Tests single brace is treated as text.
	#[test]
	fn test_parse_single_brace() {
		let result = parse_template("single { brace").unwrap();
		assert_eq!(
			result,
			vec![TemplateNode::Text("single { brace".to_string())]
		);
	}
}
