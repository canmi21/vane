IMAGE=canmi/jellyfish
TAG=$(shell git rev-parse --short HEAD)

build-and-push:
	docker buildx build \
	  --platform linux/amd64,linux/arm64 \
	  -t $(IMAGE):$(TAG) \
	  -t $(IMAGE):latest \
	  -f Dockerfile . --push

push: build-and-push
	docker pushrm $(IMAGE)

clean:
	docker buildx prune -f