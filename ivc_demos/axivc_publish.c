#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include <time.h>

int main()
{
	const char *device_path = "/dev/axivc_publisher";
	char buffer[128];
	char user_input[128];
	ssize_t bytes_read, bytes_write;

	int fd = open(device_path, O_RDWR);
	if (fd < 0)
	{
		perror("Failed to open device");
		return 1;
	}

	bytes_read = read(fd, buffer, sizeof(buffer) - 1);
	if (bytes_read < 0)
	{
		perror("Failed to read from device");
		close(fd);
		return 1;
	}

	buffer[bytes_read] = '\0';

	printf("Read from device: %s", buffer);

	printf("Enter the content to write to the device: ");
	if (fgets(user_input, sizeof(user_input), stdin) == NULL)
	{
		perror("Failed to read user input");
		close(fd);
		return 1;
	}

	// Remove newline character from user input if present
	size_t input_len = strlen(user_input);
	if (input_len > 0 && user_input[input_len - 1] == '\n')
	{
		user_input[input_len - 1] = '\0';
	}

	// Get the current timestamp
	time_t current_time = time(NULL);
	if (current_time == -1)
	{
		perror("Failed to get current time");
		close(fd);
		return 1;
	}

	struct tm *local_time = localtime(&current_time);
	if (local_time == NULL)
	{
		perror("Failed to convert time to local time");
		close(fd);
		return 1;
	}

	char timestamp[64];
	if (strftime(
			timestamp, sizeof(timestamp), "%Y-%m-%d %H:%M:%S", local_time) == 0)
	{
		fprintf(stderr, "Failed to format timestamp\n");
		close(fd);
		return 1;
	}

	// Combine user input with timestamp
	char output[256];
	snprintf(output, sizeof(output), "%s [%s]\n", user_input, timestamp);

	bytes_write = write(fd, output, strlen(output));
	if (bytes_write < 0)
	{
		perror("Failed to write to device");
		close(fd);
		return 1;
	}
	printf("Wrote %zd bytes to device\n", bytes_write);

	close(fd);
	return 0;
}