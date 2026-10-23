{
  description = "Flake for h2hc-linker";

  inputs = {
    holonix.url = "github:holochain/holonix?ref=main-0.7";

    nixpkgs.follows = "holonix/nixpkgs";
    flake-parts.follows = "holonix/flake-parts";
    rust-overlay.follows = "holonix/rust-overlay";
  };

  outputs = inputs@{ flake-parts, ... }: flake-parts.lib.mkFlake { inherit inputs; } {
    systems = builtins.attrNames inputs.holonix.devShells;
    perSystem = { inputs', pkgs, system, ... }: {
      _module.args.pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [ inputs.holonix.inputs.rust-overlay.overlays.default ];
      };

      formatter = pkgs.nixpkgs-fmt;

      devShells =
        let
          rustFromFile = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in
        {
          default = pkgs.mkShell {
            packages = (with inputs'.holonix.packages; [
              holochain
              hc
              bootstrap-srv
            ]) ++ [
              pkgs.perl
              pkgs.go
              pkgs.cmake
              pkgs.websocat  # For WebSocket testing
              rustFromFile
            ];

            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.lib.versions.major pkgs.llvmPackages.libclang.version}/include -isystem ${pkgs.glibc.dev}/include";
            RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;

            shellHook = ''
              export PS1='\[\033[1;35m\][h2hc-linker:\w]\$\[\033[0m\] '
            '';
          };
          ci = pkgs.mkShell {
            packages = [
              rustFromFile
            ];
          };
        };
    };
  };
}
