# Maintainer: Xavier B
pkgname=reef
pkgver=0.1.0
pkgrel=1
pkgdesc="Bash compatibility layer for fish shell — paste bash, it just works"
arch=('x86_64' 'aarch64')
url="https://github.com/ZStud/reef"
license=('MIT')
depends=('fish' 'bash')
makedepends=('cargo')
optdepends=(
    'ripgrep: for grep→rg tool wrapper'
    'fd: for find→fd tool wrapper'
    'sd: for sed→sd tool wrapper'
    'dust: for du→dust tool wrapper'
    'procs: for ps→procs tool wrapper'
    'eza: for ls→eza tool wrapper'
)
source=("$pkgname-$pkgver.tar.gz")
sha256sums=('SKIP')

prepare() {
    cd "$pkgname-$pkgver"
    export RUSTUP_TOOLCHAIN=stable
    cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
    cd "$pkgname-$pkgver"
    export RUSTUP_TOOLCHAIN=stable
    export CARGO_TARGET_DIR=target
    cargo build --frozen --release
}

package() {
    cd "$pkgname-$pkgver"

    # Binary
    install -Dm755 target/release/reef "$pkgdir/usr/bin/reef"

    # Core fish functions → vendor_functions.d (auto-loaded by fish)
    install -Dm644 fish/functions/export.fish "$pkgdir/usr/share/fish/vendor_functions.d/export.fish"
    install -Dm644 fish/functions/unset.fish "$pkgdir/usr/share/fish/vendor_functions.d/unset.fish"
    install -Dm644 fish/functions/declare.fish "$pkgdir/usr/share/fish/vendor_functions.d/declare.fish"
    install -Dm644 fish/functions/local.fish "$pkgdir/usr/share/fish/vendor_functions.d/local.fish"
    install -Dm644 fish/functions/readonly.fish "$pkgdir/usr/share/fish/vendor_functions.d/readonly.fish"
    install -Dm644 fish/functions/shopt.fish "$pkgdir/usr/share/fish/vendor_functions.d/shopt.fish"
    install -Dm644 fish/functions/source.fish "$pkgdir/usr/share/fish/vendor_functions.d/source.fish"
    install -Dm644 fish/functions/fish_command_not_found.fish "$pkgdir/usr/share/fish/vendor_functions.d/fish_command_not_found.fish"

    # Tool wrappers
    install -Dm644 fish/functions/tools/grep.fish "$pkgdir/usr/share/fish/vendor_functions.d/grep.fish"
    install -Dm644 fish/functions/tools/find.fish "$pkgdir/usr/share/fish/vendor_functions.d/find.fish"
    install -Dm644 fish/functions/tools/sed.fish "$pkgdir/usr/share/fish/vendor_functions.d/sed.fish"
    install -Dm644 fish/functions/tools/du.fish "$pkgdir/usr/share/fish/vendor_functions.d/du.fish"
    install -Dm644 fish/functions/tools/ps.fish "$pkgdir/usr/share/fish/vendor_functions.d/ps.fish"
    install -Dm644 fish/functions/tools/ls.fish "$pkgdir/usr/share/fish/vendor_functions.d/ls.fish"

    # conf.d (auto-loaded on fish startup)
    install -Dm644 fish/conf.d/reef.fish "$pkgdir/usr/share/fish/vendor_conf.d/reef.fish"

    # License
    install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
