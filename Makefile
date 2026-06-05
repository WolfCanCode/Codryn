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

build-ui: ensure-node
	cd ui && pnpm install && pnpm build

build-all: build-ui build

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

ensure-node:
ifeq ($(OS),Windows_NT)
	@where node >nul 2>&1 || (echo Node.js 20+ required. Install from https://nodejs.org && exit /b 1)
else
	@command -v node >/dev/null 2>&1 || { \
		echo "Node.js not found. Install Node.js 20+ from https://nodejs.org"; \
		exit 1; \
	}
	@node -e "if(parseInt(process.versions.node)<20){process.stderr.write('Node.js 20+ required\n');process.exit(1)}"
endif

test:
	SKIP_UI_BUILD=1 cargo test --all

bench:
	cargo bench -p codryn-bench

check:
	SKIP_UI_BUILD=1 cargo check --workspace

clippy:
	SKIP_UI_BUILD=1 cargo clippy --all-targets -- -D warnings

.PHONY: build build-ui build-all install ensure-cargo ensure-node test bench check clippy
