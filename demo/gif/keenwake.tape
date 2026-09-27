# Rendered by record.sh. A real run: keenwake in observe mode, the hosted Jev backend.
Output docs/img/demo.gif
Set Shell bash
Set FontSize 18
Set Width 1100
Set Height 400
Set Padding 24
Set TypingSpeed 35ms
Set Theme "GitHub Dark"

Hide
Type "cd demo/gif; export PATH=$PWD:$PATH" Enter
Type "keenwake --config keenwake.toml serve >/dev/null 2>&1 &" Enter
Type "sleep 1; clear" Enter
Sleep 2s
Show

Type "# The CPU alert flaps: fires, clears a second later. Four times." Enter
Sleep 300ms
Type "./flap.sh 1 93; ./flap.sh 2 97; ./flap.sh 3 91; ./flap.sh 4 95" Enter
Sleep 10s
Type "# Digests arrive in tomorrow's summary. Now it fires and stays up." Enter
Sleep 300ms
Type "./fire.sh 'still up, 0 min' 96" Enter
Sleep 3s
Type "sleep 240   # 4 real minutes, cut from the recording" Enter
Hide
Sleep 242s
Show
Type "./fire.sh 'still up, 4 min' 96" Enter
Sleep 5s
