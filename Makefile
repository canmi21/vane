.DEFAULT_GOAL := default

.PHONY: default just

# Default: list available recipes
default:
	@just

# Bootstrap: install just
just:
	cargo install just

# Redirect all other targets to just
%:
	@just $@
