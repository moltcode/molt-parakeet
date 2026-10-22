NAME    := molt-parakeet
VERSION := $(shell jq -r .version package.json)
CACHE   ?= $(or $(XDG_CACHE_HOME),$(HOME)/.cache)/molt-parakeet
REPO    := moltcode/$(NAME)

.PHONY: build test lint dist-darwin dist-linux model release-darwin release-model artifacts clean

build:
	cargo build --release --locked

test:
	cargo test --locked

lint:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings

# Universal, Developer ID signed and notarized macOS tarball in out/.
dist-darwin:
	./scripts/build-darwin.sh build/darwin/molt-parakeet
	./scripts/package.sh darwin build/darwin/molt-parakeet out/$(NAME)-$(VERSION)-darwin.tgz

# The host's Linux tarball (CI runs this on x64 and arm64 runners).
dist-linux:
	cargo build --release --locked
	./scripts/package.sh linux-$(if $(filter aarch64 arm64,$(shell uname -m)),arm64,x64) \
		target/release/molt-parakeet out/$(NAME)-$(VERSION)-linux-$(if $(filter aarch64 arm64,$(shell uname -m)),arm64,x64).tgz

# The model asset (out/model/*.tgz + model.json) from models.lock.json.
model:
	./scripts/model.sh "$(CACHE)" out/model

# The release workflow publishes v$(VERSION) with the Linux tarballs; this adds
# the signed macOS tarball and the catalog's artifacts.json. Assets are never
# replaced once the catalog pins them.
release-darwin: dist-darwin
	gh release upload "v$(VERSION)" out/$(NAME)-$(VERSION)-darwin.tgz --repo $(REPO)
	$(MAKE) artifacts

artifacts:
	./scripts/artifacts.sh $(VERSION) > out/artifacts.json
	cat out/artifacts.json
	gh release upload "v$(VERSION)" out/artifacts.json --repo $(REPO) --clobber

release-model: model
	gh release create "$$(jq -r .tag out/model/model.json)" out/model/*.tgz out/model/model.json --repo $(REPO) \
		--title "Model: Parakeet TDT 0.6B v3 int8 (rev $$(jq -r .revision models.lock.json))" \
		--notes "NVIDIA Parakeet TDT 0.6B v3 (CC-BY-4.0), sherpa-onnx int8 export from k2-fsa/sherpa-onnx asr-models, repacked as tar.gz with a model/ root. Checked against the sha256 pins in models.lock.json."

clean:
	rm -rf build out
