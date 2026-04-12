{
  description = "openergo workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    crane.url = "github:ipetkov/crane";
    fenix.url = "github:nix-community/fenix";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      fenix,
      flake-utils,
      ...
    }:
    {
      nixosModules.openergo =
        { pkgs, lib, ... }:
        {
          imports = [ ./nix/module.nix ];
          services.openergo.package = lib.mkDefault self.packages.${pkgs.system}.openergo-server;
          services.openergo.client.package = lib.mkDefault self.packages.${pkgs.system}.openergo-client;
        };
      nixosModules.default = self.nixosModules.openergo;
    }
    // flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;

        rustToolchain = fenix.packages.${system}.stable.defaultToolchain;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
        src = craneLib.cleanCargoSource ./.;

        commonArgs = {
          inherit src;
          strictDeps = true;

          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.autoPatchelfHook
          ];

          buildInputs = [
            pkgs.alsa-lib
            pkgs.udev
            pkgs.stdenv.cc.cc.lib
          ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        individualCrateArgs = commonArgs // {
          inherit cargoArtifacts;
          inherit (craneLib.crateNameFromCargoToml { inherit src; }) version;
          doCheck = false;
        };

        fileSetForCrate =
          crate:
          lib.fileset.unions [
            ./.cargo
            ./Cargo.toml
            ./Cargo.lock
            (craneLib.fileset.commonCargoSources ./crates/shared)
            (craneLib.fileset.commonCargoSources crate)
          ];

        srcForFileSet =
          fileset:
          lib.fileset.toSource {
            root = ./.;
            inherit fileset;
          };

        openergo-server = craneLib.buildPackage (
          individualCrateArgs
          // {
            pname = "openergo-server";
            cargoExtraArgs = "-p openergo-server";
            src = srcForFileSet (fileSetForCrate ./crates/server);
            meta = with lib; {
              description = "Openergo server";
              license = licenses.gpl3Only;
              platforms = platforms.linux;
            };
          }
        );

        openergo-client = craneLib.buildPackage (
          individualCrateArgs
          // {
            pname = "openergo-client";
            cargoExtraArgs = "-p openergo-client";
            src = srcForFileSet (lib.fileset.union
              (fileSetForCrate ./crates/client)
              ./crates/client/assets
            );
            meta = with lib; {
              description = "Openergo client";
              license = licenses.gpl3Only;
              platforms = platforms.linux;
            };
          }
        );
      in
      {
        checks = {
          inherit openergo-server openergo-client;

          workspace-clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            }
          );

          workspace-fmt = craneLib.cargoFmt {
            inherit src;
          };
        };

        packages = {
          inherit openergo-server openergo-client;
          default = openergo-server;
        };

        apps.openergo-server = flake-utils.lib.mkApp {
          drv = openergo-server;
        };

        apps.openergo-client = flake-utils.lib.mkApp {
          drv = openergo-client;
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          packages = [
            pkgs.pkg-config
          ];

          LD_LIBRARY_PATH = lib.makeLibraryPath [ pkgs.alsa-lib pkgs.udev ];

          # for rust-analyzer
          RUST_SRC_PATH = "${fenix.packages.${system}.stable.rust-src}/lib/rustlib/src/rust/library";
        };
      }
    );
}
