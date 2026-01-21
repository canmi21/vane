Name:           vane
Version:        %VERSION%
Release:        1%{?dist}
Summary:        A flow-based reverse proxy with multi-layer routing and programmable pipelines

License:        MIT
URL:            https://vaneproxy.com

Source0:        vane
Source1:        vane.service
Source2:        LICENSE
Source3:        .env
Source4:        config.tar.gz

BuildArch:      %ARCH%
Requires:       glibc

%description
Vane is a flow-based reverse proxy with multi-layer routing and programmable pipelines.

%install
# Binary
install -Dm755 %{SOURCE0} %{buildroot}/usr/bin/vane

# Systemd service
install -Dm644 %{SOURCE1} %{buildroot}/usr/lib/systemd/system/vane.service

# License
install -Dm644 %{SOURCE2} %{buildroot}/usr/share/licenses/%{name}/LICENSE

# Config directory
install -dm755 %{buildroot}/etc/vane

# Environment file
install -Dm644 %{SOURCE3} %{buildroot}/etc/vane/.env

# Extract config files
tar -xzf %{SOURCE4} -C %{buildroot}/etc/vane/

%files
%license /usr/share/licenses/%{name}/LICENSE
/usr/bin/vane
/usr/lib/systemd/system/vane.service
%dir /etc/vane
%config(noreplace) /etc/vane/.env
/etc/vane/*

%post
echo "Vane has been installed."
echo "Configuration directory: /etc/vane"
echo "To start: systemctl start vane"
echo "To enable on boot: systemctl enable vane"

%changelog
* %DATE% Vane Project - %VERSION%-1
- Release %VERSION%
