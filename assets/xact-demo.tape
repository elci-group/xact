Output assets/xact-demo.gif

Require bash

Set Shell "bash"
Set FontSize 20
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set Padding 20
Set TypingSpeed 40ms

Hide
Type "export HOME=/tmp/xact-vhs-demo && rm -rf $HOME && mkdir -p $HOME && cd /home/sal/xact && clear"
Enter
Sleep 500ms
Show

Type "./target/debug/xact"
Enter
Sleep 1500ms

Type "£ CREATE MY ~/project/src/main.rs"
Sleep 500ms
Enter
Sleep 1000ms

Type "£ SEE MY ~/project/"
Sleep 500ms
Enter
Sleep 1500ms

Type "! SPEND 40%CPU"
Sleep 500ms
Enter
Sleep 800ms

Type "£ RUN 'echo hello from a real process'"
Sleep 500ms
Enter
Sleep 1500ms

Type "exit"
Enter
Sleep 1000ms
