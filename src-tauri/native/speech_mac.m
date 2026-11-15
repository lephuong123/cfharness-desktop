// macOS speech recognition for the C4 voice bridge: SFSpeechRecognizer + AVAudioEngine.
// Compiled by build.rs (cc, -fobjc-arc) and called from src/speech/macos.rs.
//
// Threading: all state lives on one serial queue. Rust never holds a lock while calling in here and this file never
// calls back while holding anything Rust waits on, so there is no lock-order problem. Each start carries a session id
// chosen by Rust; callbacks of an older session are dropped here and again on the Rust side.

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <Speech/Speech.h>

typedef void (*cfh_speech_cb)(int kind, const char *text, int is_final, uint64_t session);

enum { CFH_RESULT = 0, CFH_END = 1, CFH_ERROR = 2, CFH_STARTED = 3 };

static dispatch_queue_t cfh_queue(void) {
  static dispatch_queue_t q;
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    q = dispatch_queue_create("io.cfharness.desktop.speech", DISPATCH_QUEUE_SERIAL);
  });
  return q;
}

static AVAudioEngine *gEngine;
static SFSpeechRecognizer *gRecognizer;
static SFSpeechAudioBufferRecognitionRequest *gRequest;
static SFSpeechRecognitionTask *gTask;
static uint64_t gSession;  // 0 = idle; only touched on cfh_queue
static cfh_speech_cb gCb;

// Requesting authorization without these Info.plist keys aborts the process (TCC), e.g. an unbundled dev binary.
static int cfh_has_usage_strings(void) {
  NSDictionary *info = [[NSBundle mainBundle] infoDictionary];
  return info[@"NSSpeechRecognitionUsageDescription"] != nil && info[@"NSMicrophoneUsageDescription"] != nil;
}

int cfh_speech_supported(const char *lang) {
  @autoreleasepool {
    if (!cfh_has_usage_strings()) return 0;
    if (lang == NULL) return 0;
    NSLocale *loc = [NSLocale localeWithLocaleIdentifier:[NSString stringWithUTF8String:lang]];
    SFSpeechRecognizer *r = [[SFSpeechRecognizer alloc] initWithLocale:loc];
    return r != nil ? 1 : 0;
  }
}

static void cfh_teardown(void) {
  if (gEngine) {
    [gEngine.inputNode removeTapOnBus:0];
    [gEngine stop];
    gEngine = nil;
  }
  if (gRequest) {
    [gRequest endAudio];
    gRequest = nil;
  }
  if (gTask) {
    [gTask cancel];
    gTask = nil;
  }
  gRecognizer = nil;
}

// Map recognizer errors onto the Web Speech error codes voice.js understands.
static const char *cfh_error_code(NSError *e) {
  if (e == nil) return "native";
  NSString *d = e.domain;
  NSInteger c = e.code;
  if ([d isEqualToString:@"kAFAssistantErrorDomain"]) {
    if (c == 1110 || c == 203) return "no-speech";       // nothing recognized / no speech detected
    if (c == 216 || c == 209 || c == 301) return "aborted"; // canceled
    if (c == 1700) return "not-allowed";
    return "network";
  }
  if ([d isEqualToString:NSURLErrorDomain]) return "network";
  return "native";
}

// Called on cfh_queue. Returns NULL on success or an error code.
static const char *cfh_begin(NSString *lang, BOOL interim, uint64_t my) {
  SFSpeechRecognizer *rec = [[SFSpeechRecognizer alloc] initWithLocale:[NSLocale localeWithLocaleIdentifier:lang]];
  if (rec == nil) return "language-not-supported";
  if (!rec.isAvailable) return "network";
  SFSpeechAudioBufferRecognitionRequest *req = [[SFSpeechAudioBufferRecognitionRequest alloc] init];
  req.shouldReportPartialResults = interim;
  if (rec.supportsOnDeviceRecognition) req.requiresOnDeviceRecognition = YES;  // keep audio on the machine
  if (@available(macOS 13.0, *)) req.addsPunctuation = YES;

  AVAudioEngine *engine = [[AVAudioEngine alloc] init];
  AVAudioInputNode *input = engine.inputNode;
  AVAudioFormat *fmt = [input outputFormatForBus:0];
  if (fmt.sampleRate <= 0 || fmt.channelCount == 0) return "audio-capture";
  SFSpeechAudioBufferRecognitionRequest *sink = req;
  [input installTapOnBus:0
              bufferSize:1024
                  format:fmt
                   block:^(AVAudioPCMBuffer *buf, AVAudioTime *when) {
                     (void)when;
                     [sink appendAudioPCMBuffer:buf];
                   }];
  [engine prepare];
  NSError *err = nil;
  if (![engine startAndReturnError:&err]) {
    [input removeTapOnBus:0];
    return "audio-capture";
  }
  gEngine = engine;
  gRequest = req;
  gRecognizer = rec;
  gTask = [rec recognitionTaskWithRequest:req
                            resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
                              NSString *text = result ? result.bestTranscription.formattedString : nil;
                              BOOL fin = result ? result.isFinal : NO;
                              dispatch_async(cfh_queue(), ^{
                                if (my != gSession) return;
                                if (text != nil) gCb(CFH_RESULT, text.UTF8String, fin ? 1 : 0, my);
                                if (error != nil || fin) {
                                  cfh_teardown();
                                  gSession = 0;
                                  if (error != nil && !fin) gCb(CFH_ERROR, cfh_error_code(error), 0, my);
                                  gCb(CFH_END, "", 0, my);  // voice.js starts a new task (like Chrome)
                                }
                              });
                            }];
  return NULL;
}

void cfh_speech_stop(void) {
  dispatch_sync(cfh_queue(), ^{
    gSession = 0;
    cfh_teardown();
  });
}

void cfh_speech_start(const char *lang, int interim, cfh_speech_cb cb, uint64_t session) {
  NSString *langS = [NSString stringWithUTF8String:(lang ? lang : "vi-VN")];
  BOOL wantInterim = interim != 0;
  uint64_t my = session;
  dispatch_sync(cfh_queue(), ^{
    cfh_teardown();
    gSession = my;
    gCb = cb;
  });
  void (^fail)(const char *) = ^(const char *code) {
    dispatch_async(cfh_queue(), ^{
      if (my != gSession) return;
      cfh_teardown();
      gSession = 0;
      cb(CFH_ERROR, code, 0, my);
    });
  };
  if (!cfh_has_usage_strings()) {
    fail("service-not-allowed");
    return;
  }
  [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus st) {
    if (st != SFSpeechRecognizerAuthorizationStatusAuthorized) {
      fail("not-allowed");
      return;
    }
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                             completionHandler:^(BOOL granted) {
                               if (!granted) {
                                 fail("not-allowed");
                                 return;
                               }
                               dispatch_async(cfh_queue(), ^{
                                 if (my != gSession) return;
                                 const char *e = cfh_begin(langS, wantInterim, my);
                                 if (e != NULL) {
                                   cfh_teardown();
                                   gSession = 0;
                                   cb(CFH_ERROR, e, 0, my);
                                 } else {
                                   cb(CFH_STARTED, "", 0, my);
                                 }
                               });
                             }];
  }];
}
