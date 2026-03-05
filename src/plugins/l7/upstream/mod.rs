// Upstream plugin now lives in vane-app
pub use vane_app::plugins::upstream::*;

pub mod hyper_client {
	pub use vane_app::plugins::upstream::hyper_client::*;
}
pub mod pool {
	pub use vane_app::plugins::upstream::pool::*;
}
pub mod tls_verifier {
	pub use vane_app::plugins::upstream::tls_verifier::*;
}
#[cfg(feature = "h3upstream")]
pub mod quinn_client {
	pub use vane_app::plugins::upstream::quinn_client::*;
}
#[cfg(feature = "h3upstream")]
pub mod quic_pool {
	pub use vane_app::plugins::upstream::quic_pool::*;
}
