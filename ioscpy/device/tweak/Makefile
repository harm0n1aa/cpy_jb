TARGET = iphone:clang:latest:14.0
ARCHS = arm64
INSTALL_TARGET_PROCESSES = SpringBoard

include $(THEOS)/makefiles/common.mk

TWEAK_NAME = ioscpyhook
ioscpyhook_INSTALL_PATH = /usr/lib/TweakInject

# Plain .mm constructor — no Logos, so the dylib does not link CydiaSubstrate.
# A Substrate dependency from Linux Theos often points at a path Dopamine cannot
# load, and ElleKit then silently skips the hook.
ioscpyhook_FILES = Tweak.mm Capture.mm StreamClient.mm Encoder.mm InputInjector.mm KeyboardSuppression.mm UiDump.mm ../daemon/Protocol.mm
ioscpyhook_CFLAGS = -fobjc-arc -I../daemon -Wno-unused-function -Wno-deprecated-declarations
ioscpyhook_FRAMEWORKS = Foundation UIKit ImageIO MobileCoreServices CoreGraphics IOSurface QuartzCore IOKit VideoToolbox CoreMedia CoreVideo

include $(THEOS)/makefiles/tweak.mk
