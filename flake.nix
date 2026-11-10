{
  description = "Zibaldone, a personal, local-first notebook";

  inputs = {
    # keep-sorted start block=yes
    crane.url = "github:ipetkov/crane";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # keep-sorted end
  };

  outputs =
    {
      crane,
      nixpkgs,
      self,
    }:
    let
      forAllSystems = nixpkgs.lib.genAttrs [
        # keep-sorted start
        "aarch64-linux"
        "x86_64-linux"
        # keep-sorted end
      ];
      packagesFor =
        pkgs:
        import ./nix/packages.nix {
          inherit pkgs;
          craneLib = crane.mkLib pkgs;
        };
    in
    {
      # keep-sorted start block=yes newline_separated=yes
      checks = forAllSystems (system: {
        inherit (packagesFor nixpkgs.legacyPackages.${system}) tests;
      });

      overlays.default = final: _: {
        inherit (packagesFor final)
          # keep-sorted start
          zib-web-clipper
          zib-web-clipper-dev
          zibaldone
          zibaldone-dev
          # keep-sorted end
          ;
      };

      packages = forAllSystems (
        system:
        let
          packages = packagesFor nixpkgs.legacyPackages.${system};
        in
        {
          # keep-sorted start
          default = self.packages.${system}.zibaldone;
          inherit (packages)
            # keep-sorted start
            zib-web-clipper
            zib-web-clipper-dev
            zibaldone
            zibaldone-dev
            # keep-sorted end
            ;
          # keep-sorted end
        }
      );
      # keep-sorted end
    };
}
