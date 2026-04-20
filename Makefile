ifeq ($(OS),Windows_NT)
INSTALL_DIR = $(USERPROFILE)\.local\bin
BINARY = codryn.exe

install: build
	@if not exist "$(INSTALL_DIR)" mkdir "$(INSTALL_DIR)"
	copy /Y target\release\$(BINARY) "$(INSTALL_DIR)\$(BINARY)"
	@echo codryn installed to $(INSTALL_DIR)\$(BINARY)

else
INSTALL_DIR = /usr/local/bin
BINARY = codryn

install: build
	sudo cp ./target/release/$(BINARY) $(INSTALL_DIR)/$(BINARY)
	sudo codesign --sign - $(INSTALL_DIR)/$(BINARY) 2>/dev/null || true
	@echo "codryn installed to $(INSTALL_DIR)/$(BINARY)"

endif

build: ensure-cargo
	cargo build --release

ensure-cargo:
ifeq ($(OS),Windows_NT)
	@where cargo >nul 2>&1 || ( \
		echo cargo not found, install Rust from https://rustup.rs && exit /b 1 \
	)
else
	@command -v cargo >/dev/null 2>&1 || { \
		echo "cargo not found, installing Rust via rustup..."; \
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; \
		. "$$HOME/.cargo/env"; \
	}
endif

.PHONY: build install ensure-cargo
