#ifndef SOFTWARE_LICENSING_POLAR_H
#define SOFTWARE_LICENSING_POLAR_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct SoftwareLicensingPolarProvider SoftwareLicensingPolarProvider;
typedef struct SoftwareLicensingPolarOperation SoftwareLicensingPolarOperation;
typedef struct SoftwareLicensingPolarResult SoftwareLicensingPolarResult;
typedef struct {
    uint32_t code;
    const uint8_t *data;
    size_t length;
} SoftwareLicensingPolarResultView;
typedef void (*SoftwareLicensingPolarCompletion)(void *context,
                                                 SoftwareLicensingPolarResult *result);

/* ABI 3 uses full SDK type names and preserves retry timing and mutation outcomes. */
/* Cancellation during a retry wait returns the received rejection and its retry
 * advice. A new unconfirmed mutation request may instead have an unknown outcome. */
uint32_t software_licensing_polar_abi_version(void);
/* Configuration JSON is copied. Create performs no network effects.
 * On failure, returns NULL and writes an owned failure into error if non-NULL. */
SoftwareLicensingPolarProvider *
software_licensing_polar_provider_create(const uint8_t *, size_t,
                                         SoftwareLicensingPolarResult **error);
/* Exclusive destruction. Started operations retain their own provider. */
void software_licensing_polar_provider_destroy(SoftwareLicensingPolarProvider *);
/* Intent JSON is copied. A non-NULL operation completes once, including errors and
 * cancellation. Callback can arrive before start returns and runs on a Rust worker.
 * Callback owns result and must not unwind. Keep context alive until completion.
 * NULL provider/callback returns NULL without completion. */
SoftwareLicensingPolarOperation *
software_licensing_polar_operation_start(const SoftwareLicensingPolarProvider *, const uint8_t *,
                                         size_t, void *, SoftwareLicensingPolarCompletion);
/* Safe on live operation; cannot undo already-started server effects. */
void software_licensing_polar_operation_cancel(const SoftwareLicensingPolarOperation *);
/* Exclusive destruction requests cancellation. Completion/context lifetime remains. */
void software_licensing_polar_operation_destroy(SoftwareLicensingPolarOperation *);
/* View remains valid only while owner lives. Copy before destroy. code=0 success;
 * code=1 stable credential-free failure JSON. Success may contain credentials. */
SoftwareLicensingPolarResultView
software_licensing_polar_result_view(const SoftwareLicensingPolarResult *);
void software_licensing_polar_result_destroy(SoftwareLicensingPolarResult *);

#ifdef __cplusplus
}
#endif
#endif
