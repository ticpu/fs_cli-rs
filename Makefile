NAME := fs-cli
BINARY := fs_cli
DOCKER := $(shell which podman 2>/dev/null || which docker)
CARGO_VERSION := $(shell grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
VERSION := v$(CARGO_VERSION)
GIT_DIRTY := $(shell git diff-index --quiet HEAD -- . 2>/dev/null || echo dirty)
GIT_TAG := $(shell git describe --exact-match --tags 2>/dev/null | grep -E '^v')
GIT_VERSION := $(shell git log --oneline . | wc -l)-$(shell git rev-parse --short HEAD)
BASE_VERSION := $(if $(GIT_DIRTY),$(VERSION)+$(GIT_VERSION),$(if $(GIT_TAG),$(VERSION),$(VERSION)+$(GIT_VERSION)))
DEB_VERSION := $(patsubst v%,%,$(BASE_VERSION))
DEB_FILE := $(NAME)_$(DEB_VERSION)_amd64.deb
IMG_BINARY := $(NAME)-binary:$(VERSION)-$(GIT_VERSION)
IMG_DEB := $(NAME)-deb:$(VERSION)-$(GIT_VERSION)
PKG := package.tmp

.PHONY: all clean binary deb

all: binary

binary: $(BINARY)

deb: $(DEB_FILE)

$(DEB_FILE): $(BINARY)-deb DEBIAN/control
	rm -rf "$(PKG)"
	install -D -m 755 -T "$(BINARY)-deb" "$(PKG)/usr/bin/$(BINARY)"
	install -D -m 644 -T fs_cli.yaml "$(PKG)/usr/share/doc/$(NAME)/examples/fs_cli.yaml"
	install -D -m 644 -T DEBIAN/control "$(PKG)/DEBIAN/control"
	sed -i "s/^Version:.*/Version: $(DEB_VERSION)/" "$(PKG)/DEBIAN/control"
	dpkg-deb --build --root-owner-group "$(PKG)" "$@"
	rm -rf "$(PKG)" "$(BINARY)-deb"

$(BINARY): Cargo.toml Cargo.lock src/*
	$(DOCKER) build --pull-always --layers=false --tag "$(IMG_BINARY)" -f Containerfile .
	$(DOCKER) rm "$(NAME)-binary" 2>/dev/null || true
	$(DOCKER) create --name "$(NAME)-binary" "$(IMG_BINARY)"
	$(DOCKER) cp "$(NAME)-binary:/app/target/release/$(BINARY)" "$(BINARY)"
	$(DOCKER) rm "$(NAME)-binary"
	$(DOCKER) image rm "$(IMG_BINARY)"

$(BINARY)-deb: Cargo.toml Cargo.lock src/*
	$(DOCKER) build --pull-always --layers=false --tag "$(IMG_DEB)" -f Containerfile.debian .
	$(DOCKER) rm "$(NAME)-deb" 2>/dev/null || true
	$(DOCKER) create --name "$(NAME)-deb" "$(IMG_DEB)"
	$(DOCKER) cp "$(NAME)-deb:/app/target/release/$(BINARY)" "$(BINARY)-deb"
	$(DOCKER) rm "$(NAME)-deb"
	$(DOCKER) image rm "$(IMG_DEB)"

clean:
	rm -rf "$(PKG)" "$(BINARY)" "$(BINARY)-deb" *.deb
