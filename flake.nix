{
  description = "Walt is a wallpaper manager for Hyprland with both a terminal UI and a native desktop GUI. It lets you browse, preview, apply, randomize, and rotate wallpapers using hyprpaper, while keeping the TUI fast for keyboard-heavy workflows and the GUI focused on preview-driven browsing.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (
      system:

      let
        pkgs = nixpkgs.legacyPackages.${system};
        waltPackage = pkgs.rustPlatform.buildRustPackage {
          pname = "walt";
          version = "0.8.0";

          src = pkgs.lib.cleanSource ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          # packages needed for building
          nativeBuildInputs = with pkgs; [ ];
          buildInputs = with pkgs; [ ];
        };

      in
      {
        packages.default = waltPackage;

        devShells.default = pkgs.mkShell {
          inputsFrom = [ waltPackage ];

          # you can add here all the dev packages you want
          nativeBuildInputs = with pkgs; [
            hyprpaper
          ];
        };
      }
    );

}
