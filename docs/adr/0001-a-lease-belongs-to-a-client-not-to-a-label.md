# A session lease belongs to the client that took it, not to the label

A lease was keyed by label, which reads as the obvious choice: a label is what the
operator configures and what every caller already has in hand. It is also wrong,
because a label is reusable and a lease is a lifetime. During a reload the label
`work` means the previous client and the replacement at the same moment, and five
separate defects came out of that single ambiguity — the worst of them released a
lock the previous client was still connected under, because publishing the
replacement dropped the only reference to the old lease and `SessionLock` holds the
operating-system lock through an open file handle.

So the store is keyed by client generation: `_active` is what each label currently
means, and `_retiring` holds every lease that is no longer active and is what keeps
its lock held. Two leases under one label is the normal state of a replacement.

## Considered options

Keying by **session identity** was the other candidate and would also have outlived
the label. It was rejected because two generations of the *same* session can briefly
coexist during a same-session rename, and an identity key cannot tell them apart —
reintroducing exactly the ambiguity being removed. The client object is unique per
generation and is already available at every call site.

## Consequences

Anything that stops referencing a lease is releasing its lock, whatever the
surrounding comment says. Tests for this area assert through a real second process
rather than through Python object identity, because an identity assertion passes on
the broken code right up until the collector runs — which is how the previous
mock-lock suite missed all five.
