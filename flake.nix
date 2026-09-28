{
  description = "Cargo project";
  inputs = {
    nixpkgs.url = "https://channels.nixos.org/nixos-unstable/nixexprs.tar.zst";
    crane.url = "github:ipetkov/crane";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, crane, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
      ];
      perSystem =
        {
          pkgs,
          system,
          config,
          ...
        }:
        {
          imports = [
            (inputs.nixpkgs.outPath + "/nixos/modules/misc/nixpkgs.nix")
            {
              nixpkgs = {
                hostPlatform = system;
                overlays = with inputs; [
                  rust-overlay.overlays.default
                ];
                # config.allowUnfree = true;
              };
            }
          ];
          config =
            let
              craneLib = (crane.mkLib pkgs).overrideToolchain (
                p:
                p.rust-bin.nightly.latest.default.override {
                  extensions = [ "rust-src" ];
                }
              );
              src = craneLib.cleanCargoSource ./.;
              commonArgs = {
                inherit src;
                strictDeps = true;

                buildInputs = [
                ];

                # Additional environment variables can be set directly
                # MY_CUSTOM_VAR = "some value";
              };
              cargoArtifacts = craneLib.buildDepsOnly commonArgs;
              my-crate = craneLib.buildPackage (
                commonArgs
                // {
                  inherit cargoArtifacts;
                }
              );
            in
            {
              checks = {
                # Build the crate as part of `nix flake check` for convenience
                inherit my-crate;

                # Run clippy (and deny all warnings) on the crate source,
                # again, reusing the dependency artifacts from above.
                #
                # Note that this is done as a separate derivation so that
                # we can block the CI if there are issues here, but not
                # prevent downstream consumers from building our crate by itself.
                my-crate-clippy = craneLib.cargoClippy (
                  commonArgs
                  // {
                    inherit cargoArtifacts;
                    cargoClippyExtraArgs = "--all-targets -- --deny warnings";
                  }
                );

                # my-crate-doc = craneLib.cargoDoc (
                #   commonArgs
                #   // {
                #     inherit cargoArtifacts;
                #     # This can be commented out or tweaked as necessary, e.g. set to
                #     # `--deny rustdoc::broken-intra-doc-links` to only enforce that lint
                #     env.RUSTDOCFLAGS = "--deny warnings";
                #   }
                # );
              };
              packages = {
                default = my-crate;
              };

              devShells.default = craneLib.devShell {
                # Inherit inputs from checks.
                checks = config.checks;

                # Additional dev-shell environment variables can be set directly
                # MY_CUSTOM_DEVELOPMENT_VAR = "something else";

                # Extra inputs can be added here; cargo and rustc are provided by default.
                packages = [
                  # pkgs.ripgrep
                ];
              };
            };
        };
    };
}
