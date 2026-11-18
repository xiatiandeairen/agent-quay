# Records docs/demo/demo.gif and demo.mp4. From the repo root:
#   docs/demo/setup.sh && docs/demo/layout.sh && vhs docs/demo/demo.tape
# The aq calls are real; build.sh and test.sh stand in for heavy builds (3 CPU cores each).
Output docs/demo/demo.gif
Output docs/demo/demo.mp4
Require tmux
Set Shell bash
Set FontSize 16
Set Width 1440
Set Height 760
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms

Hide
Type "BASH_SILENCE_DEPRECATION_WARNING=1 tmux -u -L aqdemo attach -t demo" Enter
Sleep 1.5s
Show
Sleep 1s

# Two agents start the same build, a third starts the tests.
Type "aq run -- ./build.sh" Enter
Sleep 1s
Ctrl+B
Type "b"
Type "aq run -- ./build.sh" Enter
Sleep 1s
Ctrl+B
Type "c"
Type "aq run -- ./test.sh" Enter
Sleep 17s

# Two agents each borrow a simulator. Creating and booting them takes 30–60 s, cut here.
Ctrl+B
Type "a"
Type "clear; aq hold ios-sim" Enter
Ctrl+B
Type "c"
Type "clear; aq hold ios-sim" Enter
Hide
Sleep 75s
Show
Sleep 5s
