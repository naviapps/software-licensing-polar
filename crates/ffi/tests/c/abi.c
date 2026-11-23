#define _POSIX_C_SOURCE 200809L
#include "software_licensing_polar.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
typedef struct {
    HANDLE event;
    LONG count;
    SoftwareLicensingPolarResult *result;
} Completion;
static void initialize(Completion *value) {
    memset(value, 0, sizeof *value);
    value->event = CreateEventW(NULL, TRUE, FALSE, NULL);
    assert(value->event != NULL);
}
static void complete(void *context, SoftwareLicensingPolarResult *result) {
    Completion *value = context;
    assert(InterlockedIncrement(&value->count) == 1);
    value->result = result;
    assert(SetEvent(value->event));
}
static SoftwareLicensingPolarResult *wait_result(Completion *value) {
    assert(WaitForSingleObject(value->event, 10000) == WAIT_OBJECT_0);
    assert(value->count == 1);
    return value->result;
}
static void close_completion(Completion *value) { assert(CloseHandle(value->event)); }
#else
#include <pthread.h>
#include <time.h>
typedef struct {
    pthread_mutex_t mutex;
    pthread_cond_t ready;
    unsigned count;
    SoftwareLicensingPolarResult *result;
} Completion;
static void initialize(Completion *value) {
    memset(value, 0, sizeof *value);
    assert(pthread_mutex_init(&value->mutex, NULL) == 0);
    assert(pthread_cond_init(&value->ready, NULL) == 0);
}
static void complete(void *context, SoftwareLicensingPolarResult *result) {
    Completion *value = context;
    assert(pthread_mutex_lock(&value->mutex) == 0);
    assert(++value->count == 1);
    value->result = result;
    assert(pthread_cond_signal(&value->ready) == 0);
    assert(pthread_mutex_unlock(&value->mutex) == 0);
}
static SoftwareLicensingPolarResult *wait_result(Completion *value) {
    struct timespec deadline;
    assert(clock_gettime(CLOCK_REALTIME, &deadline) == 0);
    deadline.tv_sec += 10;
    assert(pthread_mutex_lock(&value->mutex) == 0);
    while (value->count == 0)
        assert(pthread_cond_timedwait(&value->ready, &value->mutex, &deadline) == 0);
    assert(value->count == 1);
    SoftwareLicensingPolarResult *result = value->result;
    assert(pthread_mutex_unlock(&value->mutex) == 0);
    return result;
}
static void close_completion(Completion *value) {
    assert(pthread_cond_destroy(&value->ready) == 0);
    assert(pthread_mutex_destroy(&value->mutex) == 0);
}
#endif

static const char configuration[] =
    "{\"activationInstanceName\":\"Owned "
    "fixture\",\"licenseScope\":{\"organizationId\":\"11111111-1111-4111-8111-111111111111\","
    "\"benefitIds\":[\"22222222-2222-4222-8222-222222222222\"]},\"environment\":\"sandbox\","
    "\"requestTimeoutSeconds\":30,\"operationTimeoutSeconds\":60}";
static const char empty_key[] = "{\"operation\":\"activate\",\"licenseKey\":\"\"}";
static void expect_failure(SoftwareLicensingPolarResult *owner, const char *code,
                           const char *outcome) {
    SoftwareLicensingPolarResultView view = software_licensing_polar_result_view(owner);
    char expected[128];
    int length = outcome ? snprintf(expected, sizeof expected,
                                    "{\"code\":\"%s\",\"mutationOutcome\":\"%s\"}", code, outcome)
                         : snprintf(expected, sizeof expected, "{\"code\":\"%s\"}", code);
    assert(length > 0 && (size_t)length < sizeof expected);
    assert(view.code == 1 && view.data != NULL && view.length == (size_t)length);
    assert(memcmp(view.data, expected, view.length) == 0);
    software_licensing_polar_result_destroy(owner);
}
static SoftwareLicensingPolarProvider *provider(void) {
    SoftwareLicensingPolarResult *error = NULL;
    SoftwareLicensingPolarProvider *result = software_licensing_polar_provider_create(
        (const uint8_t *)configuration, strlen(configuration), &error);
    assert(result != NULL && error == NULL);
    return result;
}
int main(void) {
    assert(software_licensing_polar_abi_version() == 3);
    SoftwareLicensingPolarResultView null_view = software_licensing_polar_result_view(NULL);
    assert(null_view.code == 1 && null_view.data == NULL && null_view.length == 0);
    software_licensing_polar_result_destroy(NULL);
    software_licensing_polar_provider_destroy(NULL);
    software_licensing_polar_operation_cancel(NULL);
    software_licensing_polar_operation_destroy(NULL);
    SoftwareLicensingPolarResult *error = NULL;
    assert(software_licensing_polar_provider_create(NULL, 0, &error) == NULL);
    expect_failure(error, "invalidData", NULL);
    const char bad_config[] =
        "{\"activationInstanceName\":\"Fixture\",\"licenseScope\":{\"organizationId\":\"invalid\","
        "\"benefitIds\":[]},\"environment\":\"sandbox\"}";
    assert(software_licensing_polar_provider_create((const uint8_t *)bad_config, strlen(bad_config),
                                                    &error) == NULL);
    expect_failure(error, "invalidProviderConfiguration", NULL);
    assert(software_licensing_polar_operation_start(NULL, (const uint8_t *)empty_key,
                                                    strlen(empty_key), NULL, complete) == NULL);
    SoftwareLicensingPolarProvider *first = provider();
    assert(software_licensing_polar_operation_start(first, (const uint8_t *)empty_key,
                                                    strlen(empty_key), NULL, NULL) == NULL);
    Completion callback;
    initialize(&callback);
    SoftwareLicensingPolarOperation *operation = software_licensing_polar_operation_start(
        first, (const uint8_t *)empty_key, strlen(empty_key), &callback, complete);
    assert(operation != NULL);
    SoftwareLicensingPolarResult *result = wait_result(&callback);
    // Each owner is independent. The callback result outlives provider and operation.
    software_licensing_polar_provider_destroy(first);
    software_licensing_polar_operation_destroy(operation);
    expect_failure(result, "invalidLicenseKey", "notApplied");
    close_completion(&callback);
    // Invalid intent still returns an operation and exactly one owned completion.
    // Destroy immediately: context remains alive through that later callback.
    for (unsigned i = 0; i < 64; ++i) {
        SoftwareLicensingPolarProvider *current = provider();
        initialize(&callback);
        const uint8_t invalid[] = {0xff};
        operation = software_licensing_polar_operation_start(current, invalid, sizeof invalid,
                                                             &callback, complete);
        assert(operation != NULL);
        software_licensing_polar_operation_cancel(operation);
        software_licensing_polar_operation_destroy(operation);
        software_licensing_polar_provider_destroy(current);
        expect_failure(wait_result(&callback), "invalidData", NULL);
        close_completion(&callback);
    }
    puts("C provider/operation/result ownership and invalid-input completion passed; no License "
         "API calls.");
    return 0;
}
