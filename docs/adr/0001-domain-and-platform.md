# Domain model is separate from Windows and presentation

Tessera is developed in Rust with a native interface and no browser engine or web UI stack, including in extension presentation. Window management rules compute a placement plan and do not call Windows themselves: applying the plan belongs to the platform adapter, while the interface displays state and sends commands.

This separation makes it possible to test the rules without Windows and to use one model from both the first-party interface and extensions. The cost of this decision is the need to explicitly reconcile observed window state with the proposed plan; the plan is not treated as fact until it is applied and observed again.

Encapsulation and composition provide an object model without artificial inheritance hierarchies. Separate strategy interfaces, registries, and crates appear when real variation arises, not as empty architectural scaffolding. The tooling for the native interface has not been chosen yet.
