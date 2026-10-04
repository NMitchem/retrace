#!/bin/bash
P=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/4b336710-6279-49f2-be58-212ed366d476/scratchpad/m48-probe
cd $P/wt && { git diff; git diff --no-index /dev/null crates/retrace-box/src/probe.rs; } > $P/probe.patch; wc -l $P/probe.patch
