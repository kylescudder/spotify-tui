.PHONY: check format format-check infra-check lint test run

check: format-check lint test infra-check

format:
	cargo fmt --all

format-check:
	cargo fmt --all --check

lint:
	cargo clippy --locked --all-targets --all-features -- -D warnings

test:
	cargo test --locked --all-targets --all-features

infra-check:
	sh -n scripts/*.sh tests/*.sh
	sh tests/install-sh.sh
	sh tests/release-safety.sh
	sh tests/render-homebrew-formula.sh
	sh tests/validate-spotifyd-linux-runtime.sh
	sh tests/windows-checkout-line-endings.sh

run:
	cargo run
