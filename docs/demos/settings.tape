# Shared by the demo tapes (Source'd): the same terminal as plugii's demos.
Set Shell "bash"
Set FontFamily `"0xProto Nerd Font Mono"`
Set FontSize 15
Set Width 1280
Set Height 760
Set Padding 12
Set Framerate 24
Set TypingSpeed 45ms
Set Theme { "name": "Tomorrow Night Bright (black)", "background": "#000000", "foreground": "#eaeaea", "cursor": "#eaeaea", "selection": "#424242", "black": "#000000", "red": "#d54e53", "green": "#b9ca4a", "yellow": "#e7c547", "blue": "#7aa6da", "magenta": "#c397d8", "cyan": "#70c0b1", "white": "#eaeaea", "brightBlack": "#808080", "brightRed": "#d54e53", "brightGreen": "#b9ca4a", "brightYellow": "#e7c547", "brightBlue": "#7aa6da", "brightMagenta": "#c397d8", "brightCyan": "#70c0b1", "brightWhite": "#ffffff" }
Env TZ "UTC"
Env XDG_CACHE_HOME "/tmp/gamepadii-demo/cache"
Env XDG_DATA_HOME "/tmp/gamepadii-demo/data"

# A simulated S16 (test fixtures), so that the demo is the same each time.
Hide
Type `export PS1='\[\e[90m\]$\[\e[0m\] '; F=../../tests/fixtures; gamepadii() { ../../target/release/gamepadii --sysroot $F/sysfs/s16-xinput --mock-channel leds --mock-api $F/api/s16-fork "$@"; }; clear`
Enter
Sleep 300ms
Show
