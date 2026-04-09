{
  description = "openergo workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      flake-utils,
      ...
    }:
    {
      nixosModules.openergo =
        { pkgs, lib, ... }:
        {
          imports = [ ./nix/module.nix ];
          services.openergo.package = lib.mkDefault self.packages.${pkgs.system}.openergo-server;
        };
      nixosModules.default = self.nixosModules.openergo;
    }
    // flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;

        craneLib = crane.mkLib pkgs;
        src = craneLib.cleanCargoSource ./.;

        commonArgs = {
          inherit src;
          strictDeps = true;

          nativeBuildInputs = [
            pkgs.pkg-config
          ];

          buildInputs = [
            pkgs.udev
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
          lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./.cargo
              ./Cargo.toml
              ./Cargo.lock
              (craneLib.fileset.commonCargoSources ./crates/shared)
              (craneLib.fileset.commonCargoSources crate)
            ];
          };

        openergo-server = craneLib.buildPackage (
          individualCrateArgs
          // {
            pname = "openergo-server";
            cargoExtraArgs = "-p openergo-server";
            src = fileSetForCrate ./crates/server;
          }
        );

        openergo-client = craneLib.buildPackage (
          individualCrateArgs
          // {
            pname = "openergo-client";
            cargoExtraArgs = "-p openergo-client";
            src = fileSetForCrate ./crates/client;
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

        apps.openergo-server =
          flake-utils.lib.mkApp {
            drv = openergo-server;
          }
          // {
            meta.description = "Openergo server";
          };

        apps.openergo-client =
          flake-utils.lib.mkApp {
            drv = openergo-client;
          }
          // {
            meta.description = "Openergo client";
          };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};

          packages = [
            pkgs.pkg-config
          ];
        };
      }
    );
}
