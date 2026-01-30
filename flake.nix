{
  description = "Dev env + docker image";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      crane,
      ...
    }:
    let
      systems = flake-utils.lib.defaultSystems;
      forAllSystems = nixpkgs.lib.genAttrs systems;

      packagesFor =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          craneLib = crane.mkLib pkgs;

          manifest = (pkgs.lib.importTOML ./Cargo.toml).package;

          # Filter source to include only necessary files (ignoring flake.nix, etc.)
          src = craneLib.cleanCargoSource ./.;

          commonArgs = {
            inherit src;
            strictDeps = true;

            buildInputs = [
              pkgs.openssl
            ];

            nativeBuildInputs = [
              pkgs.pkg-config
            ];

            # SSL Env vars
            PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
            OPENSSL_DIR = "${pkgs.lib.getDev pkgs.openssl}";
            OPENSSL_LIBS_DIR = "${pkgs.lib.getLib pkgs.openssl}";
            OPENSSL_NO_VENDOR = 1;
            OPENSSL_LIB_DIR = "${pkgs.lib.getLib pkgs.openssl}/lib";
          };

          # Build *only* the dependencies (this will be cached)
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          # Build the actual crate
          pgHttpProxy = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = manifest.name;
            version = manifest.version;
          });

          archPostfix = {
            "aarch64-linux" = "-arm64";
            "x86_64-linux" = "-amd64";
            "aarch64-darwin" = "-arm64";
            "x86_64-darwin" = "-amd64";
          }.${system} or "";
        in
        rec {
          inherit pgHttpProxy;
          dockerImage = pkgs.dockerTools.buildLayeredImage {
            name = "xmadfox/pg-http-proxy-rs";
            tag = "${pgHttpProxy.version}${archPostfix}";

            contents = [
              pgHttpProxy
              pkgs.cacert # Required for SSL/TLS certificates
              pkgs.tzdata # Required for timezone data
            ];

            # Create nonroot user without requiring runAsRoot
            fakeRootCommands = ''
              mkdir -p etc
              echo "nonroot:x:1000:1000:nonroot:/home/nonroot:/bin/sh" > etc/passwd
              echo "nonroot:x:1000:" > etc/group
              mkdir -p home/nonroot
              chown 1000:1000 home/nonroot
            '';

            config = {
              Entrypoint = [ "${pgHttpProxy}/bin/pg-http-proxy" ];
              ExposedPorts = {
                "8080/tcp" = { };
              };
              User = "nonroot";
              WorkingDir = "/home/nonroot";
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

            # Ensure env vars are present in shell too
            PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
            OPENSSL_DIR = "${pkgs.lib.getDev pkgs.openssl}";

            # Other env vars for convenience
            OPENSSL_NO_VENDOR = 1;
            OPENSSL_LIB_DIR = "${pkgs.lib.getLib pkgs.openssl}/lib";
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
