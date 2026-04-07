{
  inputs = {
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      fenix,
      nixpkgs,
      ...
    }:
    let
      system = "x86_64-linux";
      pkgs = (import nixpkgs) {
        inherit system;
        overlays = [ fenix.overlays.default ];
        config.allowUnfree = true;
      };
      libPath =
        with pkgs;
        lib.makeLibraryPath [
          libGL
          libxkbcommon
          xorg.libX11
          xorg.libXcursor
          xorg.libXi
          xorg.libXrandr
          gtk3
          glib
          gdk-pixbuf
          libayatana-appindicator
          udev
        ];
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          pkgs.graphviz
          pkgs.bashInteractive
          pkgs.gdb
          pkgs.pkg-config
          pkgs.gtk3
          pkgs.librsvg
          pkgs.udev
          pkgs.python3
        ];

        nativeBuildInputs = [
          (pkgs.fenix.stable.withComponents [
            "cargo"
            "clippy"
            "rust-src"
            "rustc"
            "rustfmt"
          ])
          pkgs.mold
        ];
        LD_LIBRARY_PATH = libPath;
      };
    };
}
