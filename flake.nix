{
  description = "A local-first Spotify controller and visualizer for spotifyd";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      source = nixpkgs.lib.cleanSourceWith {
        src = ./.;
        filter = path: type:
          nixpkgs.lib.cleanSourceFilter path type
          && builtins.baseNameOf path != "target"
          && builtins.baseNameOf path != "result";
      };
      cargoManifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      packageFor = system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "spotify-tui";
          version = cargoManifest.package.version;
          src = source;

          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];

          postInstall = ''
            install -Dm644 themes/*.toml -t $out/share/spotify-tui/themes
            wrapProgram $out/bin/spotify-tui \
              --prefix PATH : ${nixpkgs.lib.makeBinPath [ pkgs.spotifyd pkgs.cava ]}
          '';

          meta = {
            description = "Local-first Spotify controller and visualizer for spotifyd";
            homepage = "https://github.com/kylescudder/spotify-tui";
            license = nixpkgs.lib.licenses.mit;
            mainProgram = "spotify-tui";
            platforms = supportedSystems;
          };
        };
    in
    {
      packages = forAllSystems (system: {
        default = packageFor system;
        spotify-tui = packageFor system;
      });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/spotify-tui";
        };
      });

      checks = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          evaluatedHomeManagerModule = nixpkgs.lib.evalModules {
            specialArgs = { inherit pkgs; };
            modules = [
              ({ lib, ... }: {
                options.home.packages = lib.mkOption {
                  type = lib.types.listOf lib.types.package;
                  default = [ ];
                };
                options.services.spotifyd = {
                  enable = lib.mkEnableOption "spotifyd test service";
                  settings = lib.mkOption {
                    type = lib.types.attrs;
                    default = { };
                  };
                };
              })
              self.homeManagerModules.default
              {
                programs.spotify-tui.enable = true;
              }
            ];
          };
        in
        {
          package = self.packages.${system}.default;
          formatting = pkgs.runCommand "spotify-tui-formatting" {
            nativeBuildInputs = [ pkgs.cargo pkgs.rustfmt ];
          } ''
            export HOME="$TMPDIR"
            cargo fmt --manifest-path ${source}/Cargo.toml --all --check
            touch $out
          '';
          home-manager-module =
            assert evaluatedHomeManagerModule.config.services.spotifyd.enable;
            assert evaluatedHomeManagerModule.config.services.spotifyd.settings.global.use_mpris;
            assert evaluatedHomeManagerModule.config.services.spotifyd.settings.global.dbus_type == "session";
            pkgs.runCommand "spotify-tui-home-manager-module" { } ''
              touch $out
            '';
        });

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.default ];
            packages = with pkgs; [ cargo clippy rust-analyzer rustfmt ];
          };
        });

      homeManagerModules.default = import ./nix/home-manager-module.nix self;
    };
}
