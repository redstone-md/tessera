# Third-party plugins are restricted independently of the replaceable interface

Tessera allows third-party extensions to replace visual modules and layout algorithms, but not to load arbitrary native libraries into the trusted core. System actions go through vetted commands; access to files, network, and other resources is granted through separate permissions.

The cost of this decision is that extensions cannot arbitrarily call Windows or run arbitrary commands. A simple "launch programs" permission can undermine sandbox constraints, so the future launch interface must distinguish approved actions from arbitrary code execution. A separate process is not, by itself, a sandbox.

The host implementation must limit extension resources, validate incoming data and placement plans, support permission revocation, and degrade back to first-party modules. Recovery and emergency handling are not replaced by plugins. Native presentation from extensions must go through a controlled UI and event description interface rather than receiving access to system pointers.

The runtime, UI format, and extension interface version have not been chosen yet; sandboxing and plugin loading are not implemented. Before choosing a runtime, the threat model must be reviewed, including arbitrary execution, data access, hangs, and spoofing of the permission interface.
