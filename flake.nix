{
  description = "Dev env + docker image";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      ...
    }:
    let
      systems = flake-utils.lib.defaultSystems;
      forAllSystems = nixpkgs.lib.genAttrs systems;

      packagesFor =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          rustPkgs = pkgs.rustPlatform;
          pgHttpProxy = rustPkgs.buildRustPackage rec {
            cargoBuildFlags = [ "" ];
            pname = "pg-http-proxy";
            version = "0.1.0";
            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            buildInputs = [
              pkgs.openssl
              pkgs.pkg-config
            ];
            PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
            OPENSSL_DIR = pkgs.lib.getDev pkgs.openssl;
            OPENSSL_LIBS_DIR = pkgs.lib.getLib pkgs.openssl;
            OPENSSL_NO_VENDOR = 1;
            OPENSSL_LIB_DIR = "${pkgs.lib.getLib pkgs.openssl}/lib";
          };
        in
        rec {
          inherit pgHttpProxy;
          dockerImage = pkgs.dockerTools.buildImage {
            name = "pg-http-proxy-rs";
            tag = "latest";

            copyToRoot = [ pgHttpProxy ];

            config = {
              Entrypoint = [ "${pgHttpProxy}/bin/pg-http-proxy" ];
              ExposedPorts = {
                "8080/tcp" = { };
              };
            };

            # TODO: build on more minimal base image
            # baseImage = someBaseImage;
          };

          default = pgHttpProxy;
        };

      devShellsFor =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
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
    in
    {
      packages = forAllSystems packagesFor;
      devShells = forAllSystems devShellsFor;
      dockerImage =
        let
          system = builtins.currentSystem;
        in
        self.packages.${system}.dockerImage;
    };
}
