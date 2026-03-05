/* src/api/src/lib.rs */

#[cfg(feature = "console")]
pub mod handlers;
#[cfg(feature = "console")]
pub mod middleware;
#[cfg(feature = "console")]
pub mod openapi;
#[cfg(feature = "console")]
pub mod response;
#[cfg(feature = "console")]
pub mod router;
#[cfg(feature = "console")]
pub mod schemas;
pub mod utils;
