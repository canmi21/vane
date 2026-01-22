IMAGE=canmi/vane
TAG=$(shell git rev-parse --short HEAD)

build:
	docker buildx build \
	--platform linux/amd64,linux/arm64 \
	-t $(IMAGE):$(TAG) \
	-t $(IMAGE):latest \
	-f Dockerfile . --push

push: build
	docker pushrm $(IMAGE)

clean:
	docker buildx prune -f