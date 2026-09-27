# Rendered by record.sh. A real run: keenwake in observe mode, the hosted Jev backend.
Output docs/img/demo.gif
Set Shell bash
Set FontSize 18
Set Width 1100
Set Height 440
Set Padding 24
Set TypingSpeed 35ms
Set Theme "GitHub Dark"

Hide
Type "cd demo/gif; export PATH=$PWD:$PATH" Enter
Type "keenwake --config keenwake.toml serve >/dev/null 2>&1 &" Enter
Type "sleep 1; clear" Enter
Sleep 2s
Show

Type "# A CPU alert that flaps: it fires, then clears on its own. Five times." Enter
Sleep 500ms
Type "for i in 1 2 3 4 5; do ./alert.sh firing; sleep 1; ./alert.sh resolved; done" Enter
Sleep 12s
Type "# What would keenwake have woken you for?" Enter
Sleep 500ms
Type "keenwake --config keenwake.toml report --since 1h" Enter
Sleep 6s
