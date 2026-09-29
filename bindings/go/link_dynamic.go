//go:build !markitai_static

package markitai

/*
#cgo CFLAGS: -I${SRCDIR}/../c
#cgo LDFLAGS: -L${SRCDIR}/../../target/release -lmarkitai_ffi
#cgo darwin LDFLAGS: -Wl,-rpath,${SRCDIR}/../../target/release
#cgo linux LDFLAGS: -Wl,-rpath,${SRCDIR}/../../target/release
*/
import "C"
