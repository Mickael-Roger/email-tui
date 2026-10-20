{
  description = "MailTUI — local-first Rust mail explorer";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
    in {
      packages = eachSystem (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
           default = pkgs.callPackage ./package.nix {};
           mailtui = self.packages.${system}.default;
        });
      nixosModules.default = import ./nixos-module.nix;
      overlays.default = final: prev: { mailtui = final.callPackage ./package.nix {}; };
      apps = eachSystem (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/mailtui";
          meta.description = "MailTUI local mail explorer";
        };
      });
      devShells = eachSystem (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [ cargo rustc rustfmt clippy pkg-config gcc python3 ];
          };
        });
    };
}
