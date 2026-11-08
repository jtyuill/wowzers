{
  description = "wowzers: World of Warcraft addon manager over the CurseForge API";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Rune on Linux: Wayland and X11 windowing (winit loads libX11 and friends, x11rb
      # loads libxcb), xkbcommon keymaps, Vulkan rendering (loaded at runtime).
      linuxGuiLibs =
        pkgs: with pkgs; [
          libxkbcommon
          vulkan-loader
          wayland
          libx11
          libxcursor
          libxi
          libxrandr
          libxcb
        ];
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage (
          {
            pname = "wowzers";
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
            src = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./crates
              ];
            };
            cargoLock = {
              lockFile = ./Cargo.lock;
              # Rune is a git dependency pinned by rev in Cargo.lock.
              allowBuiltinFetchGit = true;
            };
            cargoBuildFlags = [
              "-p"
              "wowzers"
            ];
            # Tests run in the dev shell.
            doCheck = false;
            meta.mainProgram = "wowzers";
          }
          // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.makeWrapper
            ];
            buildInputs = linuxGuiLibs pkgs;
            postFixup = ''
              wrapProgram $out/bin/wowzers \
                --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (linuxGuiLibs pkgs)}
            '';
          }
        );
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell (
          {
            packages = with pkgs; [
              cargo
              rustc
              clippy
              rustfmt
              rust-analyzer
            ];
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          }
          // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = linuxGuiLibs pkgs;
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (linuxGuiLibs pkgs);
          }
        );
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
