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
DEB_AMD64 := $(NAME)_$(DEB_VERSION)_amd64.deb
DEB_ARM64 := $(NAME)_$(DEB_VERSION)_arm64.deb
IMG := $(NAME)-build:$(VERSION)-$(GIT_VERSION)
PKG := package.tmp

.PHONY: all clean binary deb deb-amd64 deb-arm64

all: binary

binary: $(BINARY)

deb: deb-amd64 deb-arm64

deb-amd64: $(DEB_AMD64)

deb-arm64: $(DEB_ARM64)

define build-deb
	rm -rf "$(PKG)"
	install -D -m 755 -T "$<" "$(PKG)/usr/bin/$(BINARY)"
	install -D -m 644 -T fs_cli.yaml "$(PKG)/usr/share/doc/$(NAME)/examples/fs_cli.yaml"
	install -D -m 644 -T DEBIAN/control "$(PKG)/DEBIAN/control"
	sed -i -e "s/^Version:.*/Version: $(DEB_VERSION)/" -e "s/^Architecture:.*/Architecture: $(1)/" "$(PKG)/DEBIAN/control"
	dpkg-deb --build --root-owner-group "$(PKG)" "$@"
	rm -rf "$(PKG)" "$<"
endef

$(DEB_AMD64): $(BINARY)-amd64 DEBIAN/control
	$(call build-deb,amd64)

$(DEB_ARM64): $(BINARY)-arm64 DEBIAN/control
	$(call build-deb,arm64)

$(BINARY): Cargo.toml Cargo.lock src/*
	$(DOCKER) build --pull-always --layers=false --tag "$(IMG)" -f Containerfile .
	$(DOCKER) rm "$(NAME)-build" 2>/dev/null || true
	$(DOCKER) create --name "$(NAME)-build" "$(IMG)"
	$(DOCKER) cp "$(NAME)-build:/app/target/release/$(BINARY)" "$(BINARY)"
	$(DOCKER) rm "$(NAME)-build"
	$(DOCKER) image rm "$(IMG)"

$(BINARY)-amd64: Cargo.toml Cargo.lock src/*
	$(DOCKER) build --pull-always --layers=false --tag "$(IMG)" -f Containerfile.debian .
	$(DOCKER) rm "$(NAME)-build" 2>/dev/null || true
	$(DOCKER) create --name "$(NAME)-build" "$(IMG)"
	$(DOCKER) cp "$(NAME)-build:/app/target/release/$(BINARY)" "$@"
	$(DOCKER) rm "$(NAME)-build"
	$(DOCKER) image rm "$(IMG)"

$(BINARY)-arm64: Cargo.toml Cargo.lock src/*
	$(DOCKER) build --pull-always --layers=false --build-arg TARGET=aarch64-unknown-linux-gnu --tag "$(IMG)" -f Containerfile .
	$(DOCKER) rm "$(NAME)-build" 2>/dev/null || true
	$(DOCKER) create --name "$(NAME)-build" "$(IMG)"
	$(DOCKER) cp "$(NAME)-build:/app/target/release/$(BINARY)" "$@"
	$(DOCKER) rm "$(NAME)-build"
	$(DOCKER) image rm "$(IMG)"

clean:
	rm -rf "$(PKG)" "$(BINARY)" "$(BINARY)-amd64" "$(BINARY)-arm64" *.deb
