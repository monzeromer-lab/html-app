# Nix flake for HTML App (PRD §13).
#
# `nix run .` opens the launcher; `nix run . -- doc.hta` runs a document.
# `nix develop` gives a shell with every system library the build needs, which is the
# fastest way to get a working checkout on a non-Debian machine.
{
  description = "Run a single .hta file as a Linux desktop application";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # WebKitGTK, GTK, and Vulkan are linked at build time; the rest are dlopened at
        # runtime and so have to be on the library path of the wrapped binary.
        buildInputs = with pkgs; [
          webkitgtk_4_1
          gtk3
          libsoup_3
          glib
          openssl
          libxkbcommon
          wayland
          # libudev.pc — probed by libudev-sys, which `udev` and `serialport` both pull in.
          systemd
          vulkan-loader
          libGL
          xorg.libX11
          xorg.libxcb
          xorg.libXcursor
          xorg.libXi
          xorg.libXrandr
          fontconfig
          freetype
        ];

        nativeBuildInputs = with pkgs; [ pkg-config rustPlatform.bindgenHook makeWrapper ];
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "htmlapp";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          inherit buildInputs nativeBuildInputs;

          # Only the runtime binary; the other crates are libraries.
          cargoBuildFlags = [ "--bin" "htmlapp" ];

          postInstall = ''
            install -Dm644 packaging/htmlapp.desktop \
              $out/share/applications/htmlapp.desktop
            install -Dm644 packaging/htmlapp.xml \
              $out/share/mime/packages/htmlapp.xml
            install -Dm644 packaging/htmlapp.svg \
              $out/share/icons/hicolor/scalable/apps/htmlapp.svg

            # §7.2: the launcher looks here for its examples.
            mkdir -p $out/share/htmlapp/examples
            cp examples/*.hta $out/share/htmlapp/examples/

            # Vulkan and the X/Wayland client libraries are loaded at runtime, so the
            # binary needs them on its path even though they are not linked.
            wrapProgram $out/bin/htmlapp \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath buildInputs}"
          '';

          meta = with pkgs.lib; {
            description = "Run a single .hta file as a Linux desktop application";
            homepage = "https://github.com/monzeromer-lab/htmlapp";
            license = licenses.asl20;
            platforms = platforms.linux;
            mainProgram = "htmlapp";
          };
        };

        devShells.default = pkgs.mkShell {
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ (with pkgs; [
            cargo rustc rust-analyzer clippy rustfmt
            # Optional backends: layer-shell window modes, and .AppImage output.
            gtk-layer-shell appimagekit bubblewrap
          ]);

          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath buildInputs}:$LD_LIBRARY_PATH"
            echo "htmlapp dev shell — cargo build, then ./target/debug/htmlapp"
          '';
        };
      });
}
