{ lib, rustPlatform, pkg-config, makeWrapper, xdg-utils, python3 }:
rustPlatform.buildRustPackage {
  pname = "mailtui";
  version = "0.4.0";
  src = lib.cleanSourceWith {
    src = ./.;
    filter = path: type: lib.cleanSourceFilter path type && builtins.baseNameOf path != "target";
  };
  cargoLock = { lockFile = ./Cargo.lock; allowBuiltinFetchGit = true; };
  nativeBuildInputs = [ pkg-config makeWrapper ];
  nativeCheckInputs = [ python3 ];
  postInstall = ''
    wrapProgram $out/bin/mailtui --suffix PATH : ${lib.makeBinPath [ xdg-utils ]}
  '';
  meta = {
    description = "Keyboard-first local mail client with live pimdir updates";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "mailtui";
  };
}
