TARGET = iphone:clang:latest:14.0
ARCHS = arm64

include $(THEOS)/makefiles/common.mk

TOOL_NAME = ioscpyd

ioscpyd_FILES = main.mm ControlServer.mm FrameStore.mm FrameIngest.mm DaemonCapture.mm DaemonHID.mm Inject.mm SpringBoardLookup.mm Diagnostics.mm Protocol.mm ../tweak/Capture.mm ../jbcompat/Detect.mm ../jbcompat/Paths.mm
ioscpyd_CFLAGS = -fobjc-arc -I../jbcompat -I../tweak -I. -Wno-deprecated-declarations -Wno-unused-function -DIOSPY_IN_DAEMON=1
ioscpyd_FRAMEWORKS = Foundation UIKit ImageIO CoreGraphics IOSurface QuartzCore IOKit
ioscpyd_CODESIGN_FLAGS = -Sentitlements.plist
ioscpyd_INSTALL_PATH = /usr/bin

include $(THEOS)/makefiles/tool.mk
