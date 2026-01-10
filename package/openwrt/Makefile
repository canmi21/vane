include $(TOPDIR)/rules.mk

PKG_NAME:=vane
PKG_VERSION:=0.8.14
PKG_RELEASE:=1

PKG_SOURCE:=$(PKG_NAME)-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/canmi21/vane/tar.gz/v$(PKG_VERSION)?
PKG_HASH:=skip

PKG_LICENSE:=MIT
PKG_LICENSE_FILES:=LICENSE

PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_VERSION)

include $(INCLUDE_DIR)/package.mk
# include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

define Package/vane
  SECTION:=net
  CATEGORY:=Network
  TITLE:=Vane - $(PKG_VERSION)
  DEPENDS:=
  URL:=https://vaneproxy.com
endef

define Package/vane/description
  Vane is a flow-based reverse proxy with multi-layer routing and programmable pipelines.
  Version: $(PKG_VERSION)
endef

define Package/vane/conffiles
/etc/vane/
endef

define Build/Compile
	( \
		export CARGO_HOME="$(PKG_BUILD_DIR)/.cargo_home"; \
		export RUSTUP_HOME="$(PKG_BUILD_DIR)/.rustup_home"; \
		export PATH="$$CARGO_HOME/bin:$$PATH"; \
		mkdir -p "$$CARGO_HOME" "$$RUSTUP_HOME"; \
		\
		if ! command -v cargo >/dev/null 2>&1; then \
			echo "--> Cargo not found. Installing Rust toolchain..."; \
			curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path; \
		else \
			echo "--> Cargo detected."; \
		fi; \
		\
		case "$(ARCH)" in \
			x86_64) RUST_TARGET="x86_64-unknown-linux-musl" ;; \
			aarch64) RUST_TARGET="aarch64-unknown-linux-musl" ;; \
			arm)    RUST_TARGET="armv7-unknown-linux-musleabihf" ;; \
			*)      echo "Error: Unknown architecture $(ARCH)"; exit 1 ;; \
		esac; \
		echo "--> Detected Architecture: $(ARCH)"; \
		echo "--> Rust Target: $$RUST_TARGET"; \
		\
		echo "--> Ensuring target $$RUST_TARGET is installed..."; \
		$$CARGO_HOME/bin/rustup target add $$RUST_TARGET; \
		\
		LINKER_VAR="CARGO_TARGET_$$(echo $$RUST_TARGET | tr 'a-z-' 'A-Z_')_LINKER"; \
		echo "--> Using Linker: $(TARGET_CC)"; \
		export $$LINKER_VAR="$(TARGET_CC)"; \
		\
		export CC="$(TARGET_CC)"; \
		\
		echo "--> Starting Cargo Build..."; \
		cd $(PKG_BUILD_DIR) && \
		$$CARGO_HOME/bin/cargo build --release --target $$RUST_TARGET; \
		\
		echo "--> Build Complete."; \
	)
endef

define Package/vane/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_DIR) $(1)/etc/vane

	$(if $(filter x86_64,$(ARCH)), \
		$(INSTALL_BIN) $(PKG_BUILD_DIR)/target/x86_64-unknown-linux-musl/release/vane $(1)/usr/bin/,)
	$(if $(filter aarch64,$(ARCH)), \
		$(INSTALL_BIN) $(PKG_BUILD_DIR)/target/aarch64-unknown-linux-musl/release/vane $(1)/usr/bin/,)

	$(INSTALL_BIN) ./vane.init $(1)/etc/init.d/vane

	if [ -d "./config" ]; then \
		$(CP) ./config/* $(1)/etc/vane/; \
	fi

	if [ -f "./env.example" ]; then \
		$(INSTALL_CONF) ./env.example $(1)/etc/vane/.env; \
	fi
endef

define Package/vane/postinst
#!/bin/sh
if [ -z "$${IPKG_INSTROOT}" ]; then
    echo "Configuring Vane..."
    chmod 755 /etc/init.d/vane

    if [ -f /etc/init.d/uhttpd ]; then
        echo "Stopping and disabling uhttpd to free port 80/443..."
        /etc/init.d/uhttpd stop 2>/dev/null
        /etc/init.d/uhttpd disable 2>/dev/null
    fi

    /etc/init.d/vane enable
    /etc/init.d/vane restart
fi
exit 0
endef

# $(eval $(call RustBinPackage,vane))
$(eval $(call BuildPackage,vane))