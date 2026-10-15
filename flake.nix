{
  description = "blip: fast, minimal FITS / XISF image blinker for astrophotography";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # winit and wgpu dlopen these at runtime on Linux, so they are not in the binary's NEEDED
      # list. macOS uses system frameworks and needs none of them.
      runtimeLibs = pkgs: pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (with pkgs; [
        wayland
        libxkbcommon
        libx11
        libxcursor
        libxrandr
        libxi
        libxcb
        vulkan-loader
        libGL
      ]);
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "blip";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;

          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          # Make the runtime-loaded libraries findable without LD_LIBRARY_PATH.
          postFixup = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
            patchelf --add-rpath ${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)} $out/bin/blip
          '';

          meta = {
            description = "Batch Lightframe Inspection & Preview: FITS / XISF image blinker";
            license = pkgs.lib.licenses.mit;
            mainProgram = "blip";
            platforms = systems;
          };
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell ({
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = with pkgs; [ rust-analyzer clippy rustfmt ];
        } // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeLibs pkgs);
        });
      });
    };
}
