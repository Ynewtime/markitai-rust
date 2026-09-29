//go:build markitai_static && (!darwin || !arm64)

package markitai

/*
#cgo CFLAGS: -I${SRCDIR}/native/include -I${SRCDIR}/../c
#error markitai_static is currently packaged only for darwin/arm64; use the default dynamic binding on other targets
*/
import "C"
