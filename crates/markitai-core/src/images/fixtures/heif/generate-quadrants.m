// Authored test fixture generator; no runtime use or external inputs.
// clang -fobjc-arc -framework Foundation -framework CoreGraphics -framework ImageIO generate-quadrants.m -o generator
// ./generator NEW_OUTPUT_DIRECTORY
#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ImageIO/ImageIO.h>

static CGImageRef makeImage(BOOL green) {
    size_t width = 120, height = 80;
    NSMutableData *pixels = [NSMutableData dataWithLength:width * height * 4];
    unsigned char *p = pixels.mutableBytes;
    for (size_t y = 0; y < height; y++) for (size_t x = 0; x < width; x++) {
        size_t k = (y * width + x) * 4;
        BOOL right = x >= width / 2, bottom = y >= height / 2;
        p[k] = green ? 0 : (!right ? 255 : 0);
        p[k + 1] = green ? 255 : (right && !bottom ? 255 : 0);
        p[k + 2] = green ? 0 : (bottom ? 255 : 0);
        p[k + 3] = 255;
    }
    CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    CGDataProviderRef provider = CGDataProviderCreateWithCFData((__bridge CFDataRef)pixels);
    CGImageRef image = CGImageCreate(width,height,8,32,width*4,space,kCGImageAlphaLast|kCGBitmapByteOrder32Big,provider,NULL,false,kCGRenderingIntentDefault);
    CGDataProviderRelease(provider); CGColorSpaceRelease(space); return image;
}
int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc != 2) return 2;
        NSString *out = [NSString stringWithUTF8String:argv[1]];
        if ([[NSFileManager defaultManager] fileExistsAtPath:out]) return 3;
        if (![[NSFileManager defaultManager] createDirectoryAtPath:out withIntermediateDirectories:YES attributes:nil error:nil]) return 4;
        for (int variant=0; variant<3; variant++) {
            NSMutableData *data=[NSMutableData data];
            CGImageDestinationRef dest=CGImageDestinationCreateWithData((__bridge CFMutableDataRef)data,CFSTR("public.heic"),variant==2?2:1,NULL);
            if (!dest) return 5;
            CGImageRef quadrants=makeImage(NO);
            NSDictionary *options=@{(__bridge NSString *)kCGImageDestinationLossyCompressionQuality:@1.0,(__bridge NSString *)kCGImagePropertyOrientation:variant==1?@6:@1,(__bridge NSString *)kCGImagePropertyPrimaryImage:variant==2?@NO:@YES};
            CGImageDestinationAddImage(dest,quadrants,(__bridge CFDictionaryRef)options); CGImageRelease(quadrants);
            if (variant==2) {
                CGImageRef green=makeImage(YES);
                NSDictionary *primary=@{(__bridge NSString *)kCGImageDestinationLossyCompressionQuality:@1.0,(__bridge NSString *)kCGImagePropertyPrimaryImage:@YES};
                CGImageDestinationAddImage(dest,green,(__bridge CFDictionaryRef)primary); CGImageRelease(green);
            }
            BOOL ok=CGImageDestinationFinalize(dest); CFRelease(dest); if (!ok) return 6;
            NSString *name=variant==2?@"primary-second.heic":[NSString stringWithFormat:@"quadrants-orientation%d.heic",variant==1?6:1];
            if (![data writeToFile:[out stringByAppendingPathComponent:name] options:NSDataWritingWithoutOverwriting error:nil]) return 7;
            CGImageSourceRef source=CGImageSourceCreateWithData((__bridge CFDataRef)data,NULL);
            printf("%s %lu images %lu primary %lu\n",name.UTF8String,(unsigned long)data.length,(unsigned long)CGImageSourceGetCount(source),(unsigned long)CGImageSourceGetPrimaryImageIndex(source));
            CFRelease(source);
        }
    }
    return 0;
}
