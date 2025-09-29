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
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        rustPkgs = pkgs.rustPlatform;
        finalPackage = self.packages.${system}.pgHttpProxy;
      in
      {
        devShells.default = pkgs.mkShell {
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

        packages.pgHttpProxy = rustPkgs.buildRustPackage rec {
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

        # Docker image output
        dockerImage =
          let
            finalPackage = self.packages.${system}.pgHttpProxy;
          in
          pkgs.dockerTools.buildImage {
            name = "pg-http-proxy-rs";
            tag = "latest";

            # Copy the built binary into the image
            copyToRoot = [
              self.packages.${system}.pgHttpProxy
            ];

            config = {
              # Entrypoint (or Cmd) pointing to your binary
              Entrypoint = [ "${finalPackage}/bin/pg-http-proxy" ];
              ExposedPorts = {
                "8080/tcp" = { };
              };
              # Working directory, environment, etc can go here
            };

            # TODO: build on more minimal base image
            # baseImage = someBaseImage;
          };
      }
    );
}
