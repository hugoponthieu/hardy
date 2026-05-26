# A-SABR contact plan for the four-node cspcl demo.
#
# Topology (linear chain, always-available bidirectional contacts):
#   A(1) <-> B(2) <-> C(3) <-> D(4)
#
# Node 0 is the mandatory "root" allocator placeholder required by A-SABR
# (declared but unused for routing).
#
# Contact fields: contact <from> <to> <start> <end> <rate-bps> <delay-s>
# Times are Unix seconds; end=9999999999 (year 2286) keeps every link active.

node 0 root
node 1 a
node 2 b
node 3 c
node 4 d

contact 1 2 0 9999999999 10000 1
contact 2 1 0 9999999999 10000 1
contact 2 3 0 9999999999 10000 1
contact 3 2 0 9999999999 10000 1
contact 3 4 0 9999999999 10000 1
contact 4 3 0 9999999999 10000 1
