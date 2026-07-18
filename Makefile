PREFIX ?= /usr/local

RUNTIME_BINARIES := \
	target/release/openergo-server \
	target/release/openergo-client \
	target/release/openergo

.PHONY: all build install

all: build

build:
	./scripts/install-linux.sh build

install: $(RUNTIME_BINARIES)
	PREFIX="$(PREFIX)" ./scripts/install-linux.sh install

$(RUNTIME_BINARIES):
	@printf '%s\n' 'Missing prebuilt release binaries; run make as an unprivileged user first.' >&2
	@false
