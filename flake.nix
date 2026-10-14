{
  description = "win-run — a Windows Run-style launcher for nixpkgs programs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: nixpkgs.legacyPackages.${system};

      # Runtime/build libraries needed by eframe (winit + glutin/glow and the
      # Wayland/X11 backends).
      guiLibs =
        pkgs: with pkgs; [
          libGL
          libGLU
          wayland
          wayland-protocols
          libxkbcommon
          libX11
          libXcursor
          libXrandr
          libXi
          libXext
          vulkan-loader
          fontconfig
          freetype
        ];
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          libs = guiLibs pkgs;
        in
        {
          default = pkgs.mkShell {
            name = "win-run";

            nativeBuildInputs = with pkgs; [
              pkg-config
            ];

            buildInputs =
              libs
              ++ (with pkgs; [
                rustc
                cargo
                clippy
                rustfmt
                rust-analyzer
                # The launcher shells out to `nix` at runtime.
                nix
              ]);

            # Make the OpenGL/Wayland/X11 libraries discoverable when running
            # the binary straight out of `cargo run`.
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libs;
            LIBGL_DRIVERS_PATH = "${pkgs.mesa}/lib/dri";
            XDG_DATA_DIRS = "${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}";
          };
        }
      );

      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        rec {
          win-run = pkgs.rustPlatform.buildRustPackage {
            pname = "win-run";
            version = "0.1.0";

            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = with pkgs; [
              pkg-config
              makeWrapper
            ];

            buildInputs = guiLibs pkgs;

            # eframe loads some libraries (libxkbcommon, libwayland, …) via
            # dlopen at runtime, so they must be on LD_LIBRARY_PATH too.
            postFixup = ''
              wrapProgram $out/bin/win-run \
                --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath (guiLibs pkgs)}" \
                --set XKB_CONFIG_ROOT "${pkgs.xkeyboard-config}/share/X11/xkb"
            '';

            # eframe/egui tests aren't meaningful for this GUI; keep the build
            # focused on the binary.
            doCheck = false;

            meta = with pkgs.lib; {
              description = "Windows Run-style floating launcher for nixpkgs programs";
              mainProgram = "win-run";
              platforms = platforms.linux;
            };
          };

          default = win-run;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.win-run}/bin/win-run";
        };
      });
    };
}
