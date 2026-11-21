# Records the raw demo video that docs/demo/zoom.py turns into demo.gif and demo.mp4.
# Run docs/demo/record.sh from the repo root; it prepares /tmp/aq-demo and the background first.
# The aq calls are real; build.sh and test.sh stand in for heavy builds (4 CPU cores each).
# Recorded at 2x so zoom.py can zoom in up to 2x without blur.
Output "/tmp/aq-demo/raw.mp4"
Require tmux
Set Shell bash
Set FontSize 30
Set Width 2880
Set Height 1620
Set Padding 24
Set Margin 80
Set MarginFill "/tmp/aq-demo/bg.png"
Set WindowBar Colorful
Set WindowBarSize 60
Set BorderRadius 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
# VHS drops frames (and speeds the video up) when it cannot capture 2x frames in time; 15 fps keeps real time.
Set Framerate 15

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

# Two agents each borrow a simulator. aq boots them one after the other (60–120 s in all), cut here.
Ctrl+B
Type "a"
Type "clear; aq hold ios-sim" Enter
Ctrl+B
Type "c"
Type "clear; aq hold ios-sim" Enter
Hide
Sleep 120s
Show
Sleep 5s
