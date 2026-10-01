// M47 fixture (spec §3f): the dylib rpath_dyn loads through @rpath. Its install name is
// @rpath/librpath_dyn.dylib, so dyld expands @rpath to find it, which AMFI's dyld policy must allow.
int rpath_marker(void) { return 47; }
