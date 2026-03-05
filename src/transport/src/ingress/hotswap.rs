/* src/ingress/hotswap.rs */

use crate::ingress::listener;
use crate::ingress::state::Protocol;
use fancy_log::{LogLevel, log};
use live::holder::HoldEvent;
use vane_engine::config::{ConfigManager, TcpConfig, UdpConfig};

/// Starts the event loop to handle configuration changes for listeners.
pub async fn start_listener_event_loop(config: &ConfigManager) {
	let mut tcp_rx = config.listeners.tcp.subscribe();
	let mut udp_rx = config.listeners.udp.subscribe();
	let mut res_rx = config.resolvers.subscribe();
	let mut app_rx = config.applications.subscribe();

	log(LogLevel::Debug, "⚙ Starting listener event loop...");

	loop {
		tokio::select! {
				Ok(event) = tcp_rx.recv() => {
						log(LogLevel::Info, "➜ Config change signal received for TCP Listener, reloading...");
						handle_tcp_event(event).await;
				}
				Ok(event) = udp_rx.recv() => {
						log(LogLevel::Info, "➜ Config change signal received for UDP Listener, reloading...");
						handle_udp_event(event).await;
				}
				Ok(_) = res_rx.recv() => {
						log(LogLevel::Info, "➜ Config change signal received for Resolver, reloading...");
				}
				Ok(_) = app_rx.recv() => {
						log(LogLevel::Info, "➜ Config change signal received for Application, reloading...");
				}
		}
	}
}

async fn handle_tcp_event(event: HoldEvent<TcpConfig>) {
	match event {
		HoldEvent::Loaded { key, .. } => {
			if let Ok(port) = key.parse::<u16>() {
				log(LogLevel::Info, &format!("↑ PORT {port} TCP UP"));
				listener::start_listener(port, Protocol::Tcp);
			}
		}
		HoldEvent::Updated { key, old, new, .. } => {
			if let Ok(port) = key.parse::<u16>()
				&& old != new
			{
				log(LogLevel::Info, &format!("↻ PORT {port} TCP RELOAD"));
				listener::stop_listener(port, Protocol::Tcp);
				listener::start_listener(port, Protocol::Tcp);
			}
		}
		HoldEvent::Removed { key, .. } => {
			if let Ok(port) = key.parse::<u16>() {
				log(LogLevel::Info, &format!("↓ PORT {port} TCP DOWN"));
				listener::stop_listener(port, Protocol::Tcp);
			}
		}
		HoldEvent::Retained { .. } => {}
	}
}

async fn handle_udp_event(event: HoldEvent<UdpConfig>) {
	match event {
		HoldEvent::Loaded { key, .. } => {
			if let Ok(port) = key.parse::<u16>() {
				log(LogLevel::Info, &format!("↑ PORT {port} UDP UP"));
				listener::start_listener(port, Protocol::Udp);
			}
		}
		HoldEvent::Updated { key, old, new, .. } => {
			if let Ok(port) = key.parse::<u16>()
				&& old != new
			{
				log(LogLevel::Info, &format!("↻ PORT {port} UDP RELOAD"));
				listener::stop_listener(port, Protocol::Udp);
				listener::start_listener(port, Protocol::Udp);
			}
		}
		HoldEvent::Removed { key, .. } => {
			if let Ok(port) = key.parse::<u16>() {
				log(LogLevel::Info, &format!("↓ PORT {port} UDP DOWN"));
				listener::stop_listener(port, Protocol::Udp);
			}
		}
		HoldEvent::Retained { .. } => {}
	}
}
