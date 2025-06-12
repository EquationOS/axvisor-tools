#include <ivc/ulib.h>
#include <stdio.h>
#include <stdlib.h>

char message[1024];
int main(int argc, char *argv[]) {
    if (argc != 3) {
        fprintf(stderr, "Usage: %s <target_publisher_id> <channel_key>\n", argv[0]);
        return 1;
    }
    uint64_t target_publisher_id = strtoull(argv[1], NULL, 0);
    uint64_t channel_key = strtoull(argv[2], NULL, 0);

    int ret = 0;

    ivc_manager_p manager = ivc_open_manager();
    if (!manager) {
        fprintf(stderr, "Failed to open IVC manager\n");
        return 1;
    }

    ivc_subscriber_p subscriber = ivc_subscribe(manager, target_publisher_id, channel_key);
    if (!subscriber) {
        fprintf(stderr, "Failed to subscribe to channel\n");
        ret = 2;
        goto close_manager;
    }

    while (1) {
        int bytes_read = ivc_read(subscriber, message, sizeof(message) - 1);
        if (bytes_read < 0) {
            fprintf(stderr, "Failed to read from subscriber\n");
            ret = 2;
            break;
        } else if (bytes_read == 0) {
            printf("No data to read, waiting...\n");
        } else {
            message[bytes_read] = '\0'; // Null-terminate the string
            printf("Read from subscriber: %s\n", message);
        }

        printf("Total bytes read: %lu, press <q> to exit, any other key to continue.\n", subscriber->read);
        int c = getchar();
        if (c == 'q' || c == 'Q') {
            printf("Exiting subscriber loop.\n");
            break;
        }
    }

    if (ivc_unsubscribe(subscriber) < 0) {
        fprintf(stderr, "Failed to unsubscribe from channel\n");
        ret = 3;
    }
close_manager:
    if (ivc_close_manager(manager) < 0) {
        fprintf(stderr, "Failed to close IVC manager\n");
        ret = 4;
    }
    printf("IVC subscriber example finished.\n");
    return ret;
}
