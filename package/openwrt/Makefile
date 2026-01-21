include $(TOPDIR)/rules.mk

PKG_NAME:=vane
PKG_VERSION:=%VERSION%
PKG_RELEASE:=1

PKG_LICENSE:=MIT
PKG_LICENSE_FILES:=LICENSE

PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_VERSION)

include $(INCLUDE_DIR)/package.mk

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

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)
	$(CP) $(CURDIR)/target/vane $(PKG_BUILD_DIR)/vane
	chmod +x $(PKG_BUILD_DIR)/vane
endef

define Build/Compile
	true
endef

define Package/vane/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_DIR) $(1)/etc/vane
	$(INSTALL_DIR) $(1)/usr/share/licenses/vane

	$(INSTALL_BIN) $(PKG_BUILD_DIR)/vane $(1)/usr/bin/
	$(INSTALL_BIN) ./vane.init $(1)/etc/init.d/vane
	$(INSTALL_DATA) ./LICENSE $(1)/usr/share/licenses/vane/LICENSE

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
    chmod 755 /etc/init.d/vane
    if [ -f /etc/init.d/uhttpd ]; then
        /etc/init.d/uhttpd stop 2>/dev/null
        /etc/init.d/uhttpd disable 2>/dev/null
    fi
    /etc/init.d/vane enable
    /etc/init.d/vane start
fi
exit 0
endef

$(eval $(call BuildPackage,vane))