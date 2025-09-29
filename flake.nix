
{
  description = "Dev env";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs = { nixpkgs, ... }: let
    system = builtins.currentSystem;

    in {
      devShells."${system}".default = let
        pkgs = import nixpkgs {
          inherit system;
        };

      in pkgs.mkShell {
        packages = with pkgs; [
          # superior
          cargo
          rustc
          rustfmt
          openssl
          pkg-config
          clippy
        ];

        RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        };
    };
}
