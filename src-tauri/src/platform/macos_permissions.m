#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <dispatch/dispatch.h>
#include <stdbool.h>
#include <stdint.h>

uint32_t fing_microphone_authorization_status(void) {
  @try {
    return (uint32_t)[AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
  } @catch (NSException *exception) {
    return UINT32_MAX;
  }
}

bool fing_request_microphone_access(void) {
  @try {
    __block bool granted = false;
    dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);

    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                             completionHandler:^(BOOL allowed) {
                               granted = allowed;
                               dispatch_semaphore_signal(semaphore);
                             }];

    dispatch_semaphore_wait(semaphore, DISPATCH_TIME_FOREVER);
    return granted;
  } @catch (NSException *exception) {
    return false;
  }
}
