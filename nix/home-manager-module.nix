self:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.spotify-tui;
in
{
  options.programs.spotify-tui = {
    enable = lib.mkEnableOption "Spotify TUI";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.default;
      defaultText = lib.literalExpression "inputs.spotify-tui.packages.${pkgs.system}.default";
      description = "The Spotify TUI package to install.";
    };

    enableSpotifyd = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Enable spotifyd with the Linux session-MPRIS settings Spotify TUI requires.";
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      home.packages = [ cfg.package ];
    }
    (lib.mkIf cfg.enableSpotifyd {
      services.spotifyd = {
        enable = true;
        settings.global = {
          use_mpris = true;
          dbus_type = "session";
        };
      };
    })
  ]);
}
