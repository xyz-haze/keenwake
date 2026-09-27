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

Type "# api-1 fails its health check for a second, then recovers. Four times." Enter
Sleep 300ms
Type "./flap.sh 1; ./flap.sh 2; ./flap.sh 3; ./flap.sh 4" Enter
Sleep 10s
Type "# Blips go to tomorrow's digest. Now api-1 goes down and stays down." Enter
Sleep 300ms
Type "./fire.sh 'down, 0 min'" Enter
Sleep 3s
Type "sleep 240   # 4 real minutes, cut from the recording" Enter
Hide
Sleep 242s
Show
Type "./fire.sh 'still down, 4 min'" Enter
Sleep 5s
