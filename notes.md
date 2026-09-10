
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

--

## Open vs. Reveal for tools

Originating question:

> the purpose of calling open from a tool in this case is to scroll to the line. is that the way to do it? does plan9port acme have a better way?
>
> replace updates the buffer but preserves the viewport. We call open(name, line) afterward to scroll the current * row into view.
>
> Its downside is that Apex treats open as navigation: it selects the line and may warp the mouse. The call was added specifically for your “keep current change visible” request.

Recommendation:

- `Open(name, line)` works, but it is the wrong semantic level when the intent is only to keep a changed line visible.
- In Apex, `Open` is a navigation/jump operation: it may select the target, record navigation state, and warp the mouse.
- The closer plan9port acme pattern is: set dot/address, then `show`. That reveals the location without using the stronger open/jump path.
- Apex should expose an equivalent tool-facing operation, such as `Show` or `Reveal`, that brings a line/range into view without navigation side effects like mouse warp.
