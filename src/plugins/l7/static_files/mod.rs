// Static files plugin now lives in vane-app
pub use vane_app::plugins::static_files::*;

pub mod browse {
	pub use vane_app::plugins::static_files::browse::*;
}
pub mod inspect {
	pub use vane_app::plugins::static_files::inspect::*;
}
pub mod range {
	pub use vane_app::plugins::static_files::range::*;
}
pub mod router {
	pub use vane_app::plugins::static_files::router::*;
}
