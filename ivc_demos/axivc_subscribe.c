#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

struct ivc_subscribe_arg
{
	unsigned long long target_publisher_id;
	unsigned long long channel_key;
};

#define IVC_SUBSCRIBE_CHANNEL _IOW(0, 0, struct ivc_subscribe_arg)
#define IVC_UNSUBSCRIBE_CHANNEL _IOW(0, 1, struct ivc_subscribe_arg)

volatile bool keep_running = true;

void handle_signal(int sig)
{
	if (sig == SIGINT)
	{
		keep_running = false;
	}
}

int main(int argc, char *argv[])
{
	if (argc != 3)
	{
		fprintf(
			stderr, "Usage: %s <target_publisher_id> <channel_key>\n", argv[0]);
		return 1;
	}

	unsigned long long target_publisher_id = strtoull(argv[1], NULL, 0);
	unsigned long long channel_key = strtoull(argv[2], NULL, 0);

	const char *device_path = "/dev/axivc_subscriber";
	char buffer[4096];
	ssize_t bytes_read;

	int fd = open(device_path, O_RDONLY);
	if (fd < 0)
	{
		perror("Failed to open device");
		return 1;
	}

	struct ivc_subscribe_arg subscribe_arg = {
		.target_publisher_id = target_publisher_id, .channel_key = channel_key};

	if (ioctl(fd, IVC_SUBSCRIBE_CHANNEL, &subscribe_arg) < 0)
	{
		perror("Failed to subscribe to channel");
		close(fd);
		return 1;
	}

	signal(SIGINT, handle_signal);

	while (keep_running)
	{
		bytes_read = read(fd, buffer, sizeof(buffer));
		if (bytes_read < 0)
		{
			perror("Failed to read from device");
			break;
		}
		else if (bytes_read == 0)
		{
			printf("Publisher's shared memory is empty, waiting...\n");
		}
		else
		{
			buffer[bytes_read] = '\0';
			printf("Read from device: %s\n", buffer);
		}
        sleep(2);
	}

	printf("Unsubscribing from channel...\n");
	if (ioctl(fd, IVC_UNSUBSCRIBE_CHANNEL, &subscribe_arg) < 0)
	{
		perror("Failed to unsubscribe from channel");
	}

	close(fd);
	return 0;
}