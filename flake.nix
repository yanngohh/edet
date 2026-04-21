{
  description = "Flake for Holochain app development";

  inputs = {
    holonix.url = "github:holochain/holonix?ref=main-0.6";

    nixpkgs.follows = "holonix/nixpkgs";
    flake-parts.follows = "holonix/flake-parts";
  };

  outputs = inputs@{ flake-parts, ... }: flake-parts.lib.mkFlake { inherit inputs; } {
    systems = builtins.attrNames inputs.holonix.devShells;
    perSystem = { inputs', system, ... }:
      let
        pkgs = import inputs.nixpkgs {
          inherit system;
          config.allowUnfree = true;
        };
      in
      {
        formatter = pkgs.nixpkgs-fmt;

        devShells = {
          # Default shell: Holochain SDK + Essential Dev Tools
          # Python/GPU simulation is now managed via Conda/Mamba/Pixi separately.
          default = pkgs.mkShell {
            inputsFrom = [ inputs'.holonix.devShells.default ];

            packages = with pkgs; [
              nodejs_22
              husky
              binaryen
              cargo-audit
              cargo-nextest
              pkg-config
              cmake
              openssl
              zlib
            ];

            shellHook = ''
              export PS1='\[\033[1;34m\][holonix:\w]\$\[\033[0m\] '
              export LAIR_KEYSTORE_DISABLE_MLOCK=1
              ulimit -l unlimited 2>/dev/null || true

            '';
          };
        };
      };
  };
}
