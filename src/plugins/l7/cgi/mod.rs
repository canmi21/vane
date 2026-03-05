// CGI plugin now lives in vane-app
pub use vane_app::plugins::cgi::*;

pub mod executor {
	pub use vane_app::plugins::cgi::executor::*;
}
pub mod stream {
	pub use vane_app::plugins::cgi::stream::*;
}
