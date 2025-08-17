rec {
  description = "Ancient auto-clicker that still works.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
  };

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      packages.${system}.default = pkgs.stdenv.mkDerivation {
        pname = "openergo";
        version = "0.1.0";

        src = builtins.path {
          path = ./.;
          name = "source";
        };

        nativeBuildInputs = [
          pkgs.pkg-config
          pkgs.qt5.qmake
          pkgs.qt5.wrapQtAppsHook
        ];

        buildInputs = [
          pkgs.qt5.qtbase
          pkgs.qt5.qtmultimedia
          pkgs.qt5.qtx11extras
          pkgs.xorg.libXi
          pkgs.xorg.libXtst
          pkgs.xorg.libxcb
        ];

        installPhase = ''
          runHook preInstall
          mkdir -p $out/bin
          install -m 755 openergo $out/bin/openergo
          runHook postInstall
        '';

        meta = with pkgs.lib; {
          inherit description;
          homepage = "https://github.com/wpbrown/openergo";
          license = licenses.gpl3;
          platforms = platforms.linux;
        };
      };

      apps.${system}.default = {
        type = "app";
        program = "${self.packages.${system}.default}/bin/openergo";
      };

      devShells.${system}.default = pkgs.mkShell {
        inputsFrom = [ self.packages.${system}.default ];
      };
    };
}
