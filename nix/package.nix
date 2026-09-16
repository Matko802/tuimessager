{
  lib,
  stdenv,
  craneLib,
  pkg-config,
  sqlite,
  src,
  pname,
  version,
  description,
}:
let
  cargoExtraArgs =
    if pname == "tuimessager-server" then "--locked -p tuimessager-server"
    else "--locked -p tuimessager-tui";
in
craneLib.buildPackage {
  inherit src pname version cargoExtraArgs;

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ sqlite ];

  doCheck = false;

  meta = {
    inherit description;
    license = lib.licenses.gpl3Only;
    mainProgram = if pname == "tuimessager-server" then "tuimessager-server" else "tuimessager";
    platforms = lib.platforms.unix;
  };
}
