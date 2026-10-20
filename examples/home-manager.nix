# Pass the mailtui flake input to this Home Manager module as `inputs.mailtui`.
{ pkgs, inputs, ... }: {
  home.packages = [ inputs.mailtui.packages.${pkgs.stdenv.hostPlatform.system}.default ];
  xdg.configFile."mailtui/config.toml".text = ''
    [ui]
    theme = "k9s-dark"

    [viewer]
    mode = "auto"
    browser_command = ["${pkgs.xdg-utils}/bin/xdg-open", "{url}"]

    [[accounts]]
    id = "personal"
    label = "Personal"
    email = "me@example.com"
    store = "/home/your-user/actual-pimdir-store"
  '';
}
