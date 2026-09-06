
remotes should work as long as you can supply a way to execute a command on the remote and connect stdin/out/err. 

for example, fly.io's sprites can run commands in this way:

sprite exec -s apex-test -- bash -c "ls /"

this way, we can execute a bash script that first starts a little control channel: check if there is a running session and attach to it, otherwise upload the binary and start it (uploading has to be done via the control channel too). 

then attach to the binary.

we should allow the user to configure *providers* that help make these connections.  ssh provider is straightforward., but we can then also have a 'sprite' provider, etc. a provider can take a single argument (e.g., hostname for ssh, sprite name for above), but that should suffice to get going.



--


let's port the rc shell. and run by default in the rc shell.  that will allow us to also define $%, $samfile, etc.

select, then release outside, shoudl also B3 correctly.

plumber!

fully document `apex` command.

terminal titles
--

scroll responsiveness (w/ trackpad)

in plan9port acme, i can type a path, e.g., hit 'New', enter some content, type the path name, and then hit 'Put'. this isn't available in apex.

then let's see what it would take to implement 'rc'

--

include 'rc' in the distribution, and use it in place of 'sh' for running commands. https://github.com/mariusae/rustrc. as with plan9port acme, define $%, $samfile, (others?), when running commands -- regardless of whether it is through rc, or directly. also make sure that 'rc' is on the path on the server, like we do with 'apex'. so we'll need some way to upload both files on first bootstrap.

--

server version vs. client version. at least display.



term: on keyboard input, scroll to the end automatically.