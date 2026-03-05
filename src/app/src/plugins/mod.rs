#[cfg(feature = "tls")]
pub mod upstream;

pub mod response;

#[cfg(feature = "cgi")]
pub mod cgi;

#[cfg(feature = "static")]
pub mod static_files;
