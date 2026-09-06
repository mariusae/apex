
remotes should work as long as you can supply a way to execute a command on the remote and connect stdin/out/err. 

for example, fly.io's sprites can run commands in this way:

sprite exec -s apex-test -- bash -c "ls /"

this way, we can execute a bash script that first starts a little control channel: check if there is a running session and attach to it, otherwise upload the binary and start it (uploading has to be done via the control channel too). 

then attach to the binary.

we should allow the user to configure *providers* that help make these connections.  ssh provider is straightforward., but we can then also have a 'sprite' provider, etc. a provider can take a single argument (e.g., hostname for ssh, sprite name for above), but that should suffice to get going.



--


in plan9port acme, i can type a path, e.g., hit 'New', enter some content, type the path name, and then hit 'Put'. this isn't available in apex.


let's port the rc shell. and run by default in the rc shell.


select, then release outside, shoudl also B3 correctly.


plumber!

fully document `apex` command.

terminal titles

mac keyboard shorcuts: cmd-s for Put, cmd-w for Del, cmd-n for new, cmd-shift-n for new window

--

scroll responsiveness (w/ trackpad)

--

the mouse no longer renders the square handle when selecting a window

--

when switching windows (command-`), let's restore the cursor to the last position in that window, e.g., so i can use command-` to swap, and always restore the cursor correctly