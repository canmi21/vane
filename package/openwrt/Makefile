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
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

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

define Package/vane/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_DIR) $(1)/etc/vane

	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/vane $(1)/usr/bin/
	$(INSTALL_BIN) ./vane.init $(1)/etc/init.d/vane

	if [ -d "./config" ]; then \
		$(CP) ./config/* $(1)/etc/vane/; \
	fi

	if [ -f "./.env" ]; then \
		$(INSTALL_CONF) ./.env $(1)/etc/vane/.env; \
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

$(eval $(call RustBinPackage,vane))
$(eval $(call BuildPackage,vane))