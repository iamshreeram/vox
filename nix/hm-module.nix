# Home-manager module for Vox speech-to-text
#
# Provides a systemd user service for autostart.
# Usage: imports = [ vox.homeManagerModules.default ];
#        services.vox.enable = true;
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.vox;
in
{
  options.services.vox = {
    enable = lib.mkEnableOption "Vox speech-to-text user service";

    package = lib.mkOption {
      type = lib.types.package;
      defaultText = lib.literalExpression "vox.packages.\${system}.vox";
      description = "The Vox package to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.vox = {
      Unit = {
        Description = "Vox speech-to-text";
        After = [ "graphical-session.target" ];
        PartOf = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = "${cfg.package}/bin/vox";
        Restart = "on-failure";
        RestartSec = 5;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
