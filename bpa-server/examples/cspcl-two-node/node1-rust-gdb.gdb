set breakpoint pending on
set print pretty on

# CSPCL ingress: recv_bundle() returned a complete bundle.
break /home/hugo/code/hardy/cspcl/src/cla.rs:31

# CSPCL ingress: right before the CLA dispatches into the BPA.
break /home/hugo/code/hardy/cspcl/src/cla.rs:51

# BPA CLA sink entry.
break /home/hugo/code/hardy/bpa/src/cla/registry.rs:73

# BPA bundle ingest: parse/store/ingress metadata.
break /home/hugo/code/hardy/bpa/src/dispatcher/dispatch.rs:19

# BPA routing decision.
break /home/hugo/code/hardy/bpa/src/dispatcher/dispatch.rs:332

# Optional: local service/application delivery.
break /home/hugo/code/hardy/bpa/src/dispatcher/local.rs:188
break /home/hugo/code/hardy/bpa/src/dispatcher/local.rs:223

# Optional: forwarding back out over CSP.
break /home/hugo/code/hardy/cspcl/src/cla.rs:119

run -c /home/hugo/code/hardy/bpa-server/examples/cspcl-two-node/node1.toml
