/* src/primitives/src/common/sys/system.rs */

#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "windows"))]
use std::process::Command;

/// Returns the free memory of the system in bytes.
/// Supported platforms: Linux, macOS, FreeBSD.
#[must_use]
pub fn get_free_memory() -> Option<u64> {
	#[cfg(target_os = "linux")]
	{
		use std::fs;
		if let Ok(content) = fs::read_to_string("/proc/meminfo") {
			for line in content.lines() {
				if line.starts_with("MemAvailable:") || line.starts_with("MemFree:") {
					let parts: Vec<&str> = line.split_whitespace().collect();
					if parts.len() >= 2 {
						if let Ok(kb) = parts[1].parse::<u64>() {
							return Some(kb * 1024);
						}
					}
				}
			}
		}
		None
	}

	#[cfg(target_os = "macos")]
	{
		// macOS: sysctl -n vm.page_free_count * sysctl -n vm.pagesize
		let page_free = execute_sysctl("vm.page_free_count")?;
		let page_size = execute_sysctl("hw.pagesize")?;
		Some(page_free * page_size)
	}

	#[cfg(target_os = "windows")]
	{
		// Windows: wmic OS get FreePhysicalMemory /Value
		// Output format:
		//
		// FreePhysicalMemory=1234567
		//
		let output =
			Command::new("wmic").args(["OS", "get", "FreePhysicalMemory", "/Value"]).output().ok()?;

		if output.status.success() {
			let content = String::from_utf8_lossy(&output.stdout);
			for line in content.lines() {
				if let Some(val_str) = line.trim().strip_prefix("FreePhysicalMemory=") {
					if let Ok(kb) = val_str.parse::<u64>() {
						return Some(kb * 1024);
					}
				}
			}
		}
		None
	}

	#[cfg(target_os = "freebsd")]
	{
		// FreeBSD: vm.stats.vm.v_free_count * hw.pagesize
		let page_free = execute_sysctl("vm.stats.vm.v_free_count")?;
		let page_size = execute_sysctl("hw.pagesize")?;
		Some(page_free * page_size)
	}

	#[cfg(not(any(
		target_os = "linux",
		target_os = "macos",
		target_os = "freebsd",
		target_os = "windows"
	)))]
	None
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn execute_sysctl(query: &str) -> Option<u64> {
	let output = Command::new("sysctl").args(["-n", query]).output().ok()?;

	if output.status.success() {
		let s = String::from_utf8_lossy(&output.stdout).trim().to_owned();
		return s.parse::<u64>().ok();
	}
	None
}

/// Validates if the current platform supports adaptive memory management.
#[must_use]
pub fn is_adaptive_supported() -> bool {
	get_free_memory().is_some()
}
