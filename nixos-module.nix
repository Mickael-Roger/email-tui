{ config, lib, pkgs, ... }:
let
  cfg = config.programs.mailtui;
  toml = pkgs.formats.toml {};
in {
  options.programs.mailtui = {
    enable = lib.mkEnableOption "MailTUI";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix {};
      description = "MailTUI package to install.";
    };
    settings = lib.mkOption {
      type = toml.type;
      default = {};
      description = "System default configuration, written to /etc/xdg/mailtui/config.toml. Mail credentials belong in Neverest; LLM API keys are referenced through environment variables.";
    };
  };
  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    environment.etc = lib.mkIf (cfg.settings != {}) {
      "xdg/mailtui/config.toml".source = toml.generate "mailtui.toml" cfg.settings;
    };
  };
}
