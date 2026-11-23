#include <CSoftwareLicensingPolar/software_licensing_polar.h>
#include <assert.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    assert(software_licensing_polar_abi_version() == 3);
    // Invalid Polar scope fails before the provider constructs its HTTP client.
    const char config[] =
        "{\"activationInstanceName\":\"Fixture\",\"licenseScope\":{\"organizationId\":\"invalid\","
        "\"benefitIds\":[]},\"environment\":\"sandbox\"}";
    SoftwareLicensingPolarResult *failure = NULL;
    SoftwareLicensingPolarProvider *provider = software_licensing_polar_provider_create(
        (const uint8_t *)config, sizeof config - 1, &failure);
    assert(provider == NULL && failure != NULL);

    SoftwareLicensingPolarResultView view = software_licensing_polar_result_view(failure);
    const char expected[] = "{\"code\":\"invalidProviderConfiguration\"}";
    assert(view.code == 1 && view.data != NULL && view.length == sizeof expected - 1);
    assert(memcmp(view.data, expected, view.length) == 0);
    software_licensing_polar_result_destroy(failure);
    puts("Standalone installed C Polar consumer passed configuration preflight.");
    return 0;
}
