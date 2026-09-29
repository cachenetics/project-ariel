belicose:~/project-ariel$ doas ./install.sh
doas (infinitevalence@belicose.endlessdelve.com) password:
Detected distro: alpine
building release binary...
Compiling apu v0.1.0 (/home/infinitevalence/project-ariel/arieltune/crates/apu)
error[E0599]: the method `to_string` exists for enum `std::option::Option<&str>`, but its trait bounds were not satisfied
--> crates/apu/src/persist.rs:222:4
	|
218 |       let exec_start = body
	|  ______________________-
219 | |         .lines()
220 | |         .find(|l| l.starts_with("ExecStart="))
221 | |         .map(|l| &l[10..])
222 | |         .to_string();
	| |         -^^^^^^^^^ method cannot be called on `std::option::Option<&str>` due to unsatisfied trait bounds
	| |_________|
	|
	|
= note: the following trait bounds were not satisfied:
`std::option::Option<&str>: std::fmt::Display`
which is required by `std::option::Option<&str>: ToString`
note: the method `to_string` exists on the type `&str`
--> /rustc/31fca3adb283cc9dfd56b49cdee9a96eb9c96ffd/library/alloc/src/string.rs:2880:4
help: consider using `Option::expect` to unwrap the `&str` value, panicking if the value is an `Option::None`
	|
221 |         .map(|l| &l[10..]).expect("REASON")
	|                           +++++++++++++++++

error[E0308]: mismatched types
--> crates/apu/src/persist.rs:240:16
	|
240 |           out.push_str(format!(
	|  ______________________^
241 | |             "\tstart-stop-daemon --start --pidfile \"$pidfile\"\n\t--background --exec {}\n",
242 | |             bin
243 | |         ));
	| |_________^ expected `&str`, found `String`

error[E0308]: mismatched types
--> crates/apu/src/persist.rs:245:16
	|
245 |           out.push_str(format!(
	|  ______________________^
246 | |             "\tstart-stop-daemon --start --pidfile \"$pidfile\"\n\t--background --exec {} -- {}\n",
247 | |             bin, args
248 | |         ));
	| |_________^ expected `&str`, found `String`

Some errors have detailed explanations: E0308, E0599.
For more information about an error, try `rustc --explain E0308`.
error: could not compile `apu` (lib) due to 3 previous errors
))
))
